use cowbot_core::event::Event;
use parking_lot::RwLock;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::debug;

/// Receives every published event, in order.
type Recorder = Arc<dyn Fn(&Event) + Send + Sync>;

/// Broadcast event bus for multi-agent messaging and pub/sub.
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<Event>,
    /// Set only by [`EventBus::recording`]; production buses leave it empty.
    recorder: Option<Recorder>,
}

impl EventBus {
    /// Create a new event bus with the given broadcast channel capacity.
    pub fn new(channel_capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(channel_capacity);
        Self {
            sender,
            recorder: None,
        }
    }

    /// Default bus with a 1024-event broadcast capacity.
    pub fn default_bus() -> Self {
        Self::new(1024)
    }

    /// A bus that also hands every published event to `sink`.
    ///
    /// Tests use this to assert on the full event sequence after the work has
    /// finished; production code has no reader for one.
    pub fn recording(
        channel_capacity: usize,
        sink: impl Fn(&Event) + Send + Sync + 'static,
    ) -> Self {
        let (sender, _) = broadcast::channel(channel_capacity);
        Self {
            sender,
            recorder: Some(Arc::new(sink)),
        }
    }

    /// Publish an event to all active subscribers.
    ///
    /// Returns the number of live subscribers that received the event.
    pub fn publish(&self, event: Event) -> usize {
        debug!(event_id = %event.id, "Publishing event to bus");
        if let Some(recorder) = &self.recorder {
            recorder(&event);
        }
        // A send error means there are no active receivers, which is fine for pub/sub.
        self.sender.send(event).unwrap_or(0)
    }

    /// Subscribe to live events broadcast on this bus.
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.sender.subscribe()
    }
}

/// A bus plus the events it published, for tests that assert on the sequence.
#[derive(Clone)]
pub struct RecordingBus {
    bus: EventBus,
    published: Arc<RwLock<Vec<Event>>>,
}

impl RecordingBus {
    pub fn new() -> Self {
        let published = Arc::new(RwLock::new(Vec::new()));
        let sink = published.clone();
        let bus = EventBus::recording(1024, move |event| sink.write().push(event.clone()));
        Self { bus, published }
    }

    pub fn bus(&self) -> &EventBus {
        &self.bus
    }

    /// Every event published so far, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.published.read().clone()
    }
}

impl Default for RecordingBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cowbot_core::event::EventPayload;

    #[tokio::test]
    async fn test_bus_publish_and_receive() {
        let bus = EventBus::new(16);
        let mut rx = bus.subscribe();

        let event = Event::new(EventPayload::SystemNotice {
            message: "CowBot engine initialized".to_string(),
        });

        let recipients = bus.publish(event.clone());
        assert_eq!(recipients, 1);

        let received = rx.recv().await.unwrap();
        assert_eq!(received.id, event.id);
    }

    #[tokio::test]
    async fn publishing_without_subscribers_is_not_an_error() {
        let bus = EventBus::new(16);
        let event = Event::new(EventPayload::SystemNotice {
            message: "Notice".to_string(),
        });
        assert_eq!(bus.publish(event), 0);
    }

    #[test]
    fn the_recording_bus_keeps_every_event() {
        let recording = RecordingBus::new();
        for index in 0..3 {
            recording
                .bus()
                .publish(Event::new(EventPayload::SystemNotice {
                    message: format!("Notice {index}"),
                }));
        }
        assert_eq!(recording.events().len(), 3);
    }
}

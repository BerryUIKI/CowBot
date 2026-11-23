//! The protobuf frames Feishu's long connection speaks (ADR-0061).
//!
//! The transport is not JSON. Each frame is a `pbbp2.Frame` message — a varint
//! length prefix, then fields — and the payload inside it is JSON (sometimes
//! base64-wrapped JSON). Only nine field numbers of one message type are
//! involved, all of them varints or length-delimited strings, so the codec here
//! is written by hand rather than pulling in `prost` and a build-time code
//! generator for a single struct.
//!
//! The format, from `larksuite/oapi-sdk-go`'s `ws/pbbp2.pb.go`:
//!
//! ```text
//! Frame {
//!   1: seq_id           varint
//!   2: log_id           varint
//!   3: service          varint (int32)
//!   4: method           varint (int32)  0 = control, 1 = data
//!   5: headers          repeated Header { 1: key, 2: value }
//!   6: payload_encoding string
//!   7: payload_type     string
//!   8: payload          bytes
//!   9: log_id_new       string
//! }
//! ```
//!
//! The decoder is deliberately tolerant: an unknown field is skipped by its wire
//! type rather than rejected, because Feishu adds fields and a frame that fails
//! to parse is a dropped message with no log line to explain it.

/// The method number of a control frame (ping, pong).
pub const METHOD_CONTROL: i32 = 0;

/// The method number of a data frame (an event, or a card callback).
pub const METHOD_DATA: i32 = 1;

/// The header key that names the frame's kind.
pub const HEADER_TYPE: &str = "type";

/// One frame on the wire.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frame {
    pub seq_id: u64,
    pub log_id: u64,
    pub service: i32,
    pub method: i32,
    pub headers: Vec<(String, String)>,
    pub payload_encoding: String,
    pub payload_type: String,
    pub payload: Vec<u8>,
    pub log_id_new: String,
}

impl Frame {
    /// The value of one header, if it is present.
    pub fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// The frame's kind: `event`, `card`, `ping`, or `pong`.
    pub fn kind(&self) -> Option<&str> {
        self.header(HEADER_TYPE)
    }

    /// The payload as text.
    ///
    /// The encoding header says how the bytes are wrapped; an event's payload is
    /// plain UTF-8 JSON, and a response's is base64 of JSON. Only the plain form
    /// is read here, and an encoding this does not know returns `None` rather
    /// than mangled text.
    pub fn payload_text(&self) -> Option<&str> {
        match self.payload_encoding.as_str() {
            "" | "json" => std::str::from_utf8(&self.payload).ok(),
            _ => None,
        }
    }

    /// A ping frame, which keeps the connection alive and proves it is alive.
    pub fn ping(service: i32) -> Self {
        Self {
            service,
            method: METHOD_CONTROL,
            headers: vec![(HEADER_TYPE.to_string(), "ping".to_string())],
            ..Self::default()
        }
    }

    /// The answer to one data frame.
    ///
    /// `code` is 200 when the handler succeeded. The body is JSON of
    /// `{code, headers, data}`; `data` is a base64 string, and for a card
    /// callback it is that string's contents — the toast or card to show — that
    /// the client reads.
    pub fn response(frame: &Frame, code: u16, data: Option<&str>) -> Self {
        let encoded = data.map(|data| base64(data.as_bytes()));
        let body = serde_json::json!({
            "code": code,
            "headers": {},
            "data": encoded,
        });
        Self {
            seq_id: frame.seq_id,
            log_id: frame.log_id,
            service: frame.service,
            method: frame.method,
            headers: vec![(HEADER_TYPE.to_string(), "event".to_string())],
            payload_encoding: "json".to_string(),
            payload_type: String::new(),
            payload: body.to_string().into_bytes(),
            log_id_new: String::new(),
        }
    }

    /// Encode this frame with its varint length prefix.
    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::new();
        write_varint_field(&mut body, 1, self.seq_id);
        write_varint_field(&mut body, 2, self.log_id);
        write_varint_field(&mut body, 3, self.service as u64);
        write_varint_field(&mut body, 4, self.method as u64);
        for (key, value) in &self.headers {
            let mut header = Vec::new();
            write_string_field(&mut header, 1, key);
            write_string_field(&mut header, 2, value);
            write_bytes_field(&mut body, 5, &header);
        }
        if !self.payload_encoding.is_empty() {
            write_string_field(&mut body, 6, &self.payload_encoding);
        }
        if !self.payload_type.is_empty() {
            write_string_field(&mut body, 7, &self.payload_type);
        }
        if !self.payload.is_empty() {
            write_bytes_field(&mut body, 8, &self.payload);
        }
        if !self.log_id_new.is_empty() {
            write_string_field(&mut body, 9, &self.log_id_new);
        }

        let mut encoded = Vec::with_capacity(body.len() + 4);
        write_varint(&mut encoded, body.len() as u64);
        encoded.extend_from_slice(&body);
        encoded
    }

    /// Decode one frame, ignoring its length prefix if one is present.
    ///
    /// The prefix is optional because a caller may have already stripped it: the
    /// SDKs read the length to know how many bytes to wait for, and the frame
    /// itself does not contain it.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        Self::decode_body(strip_length_prefix(bytes))
    }

    /// Decode a frame body, with no length prefix.
    ///
    /// Separate from [`Self::decode`] because a caller that has already split the
    /// stream must not have its bytes run through the prefix heuristic — a body
    /// whose first byte happens to look like a matching length would lose its
    /// first field.
    pub fn decode_body(body: &[u8]) -> Option<Self> {
        let mut frame = Frame::default();
        let mut cursor = 0usize;
        while cursor < body.len() {
            let (key, read) = read_varint(&body[cursor..]).ok()?;
            cursor += read;
            let field = (key >> 3) as u32;
            let wire = (key & 0b111) as u8;
            match wire {
                // Varint
                0 => {
                    let (value, read) = read_varint(&body[cursor..]).ok()?;
                    cursor += read;
                    match field {
                        1 => frame.seq_id = value,
                        2 => frame.log_id = value,
                        3 => frame.service = value as i32,
                        4 => frame.method = value as i32,
                        _ => {}
                    }
                }
                // Length-delimited
                2 => {
                    let (length, read) = read_varint(&body[cursor..]).ok()?;
                    cursor += read;
                    let end = cursor.checked_add(length as usize)?;
                    if end > body.len() {
                        return None;
                    }
                    let slice = &body[cursor..end];
                    cursor = end;
                    match field {
                        5 => frame.headers.push(decode_header(slice)?),
                        6 => frame.payload_encoding = String::from_utf8_lossy(slice).into_owned(),
                        7 => frame.payload_type = String::from_utf8_lossy(slice).into_owned(),
                        8 => frame.payload = slice.to_vec(),
                        9 => frame.log_id_new = String::from_utf8_lossy(slice).into_owned(),
                        _ => {}
                    }
                }
                // Fixed-width fields do not appear in this message; skipping them
                // needs their width, which is what the two cases below give.
                1 => cursor = cursor.checked_add(8)?,
                5 => cursor = cursor.checked_add(4)?,
                _ => return None,
            }
        }
        Some(frame)
    }
}

/// Decode a `Header` submessage.
fn decode_header(bytes: &[u8]) -> Option<(String, String)> {
    let mut key = String::new();
    let mut value = String::new();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        let (tag, read) = read_varint(&bytes[cursor..]).ok()?;
        cursor += read;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0b111) as u8;
        if wire != 2 {
            // No other wire type appears in this message; a varint here would be
            // a field Feishu added, which is skipped by the caller's default.
            return None;
        }
        let (length, read) = read_varint(&bytes[cursor..]).ok()?;
        cursor += read;
        let end = cursor.checked_add(length as usize)?;
        if end > bytes.len() {
            return None;
        }
        let text = String::from_utf8_lossy(&bytes[cursor..end]).into_owned();
        cursor = end;
        match field {
            1 => key = text,
            2 => value = text,
            _ => {}
        }
    }
    Some((key, value))
}

/// Skip a length prefix if the bytes start with one that matches.
///
/// A frame read straight off a length-delimited stream starts with its own
/// length, and a frame handed over by a caller that already split it does not.
/// Distinguishing them: a varint prefix whose value is exactly the remaining
/// length is a prefix. Anything else is a field tag, and the first field of a
/// frame is a varint with field number 1 — tag `0x08` — which cannot be mistaken
/// for a length that equals the rest.
fn strip_length_prefix(bytes: &[u8]) -> &[u8] {
    if let Ok((length, read)) = read_varint(bytes) {
        if read < bytes.len() && read + length as usize == bytes.len() {
            return &bytes[read..];
        }
    }
    bytes
}

fn read_varint(bytes: &[u8]) -> Result<(u64, usize), ()> {
    let mut value = 0u64;
    let mut shift = 0u32;
    for (index, byte) in bytes.iter().enumerate() {
        // A varint is at most ten bytes; anything longer is not one.
        if shift >= 64 {
            return Err(());
        }
        value |= u64::from(byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
        shift += 7;
    }
    Err(())
}

fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn write_varint_field(out: &mut Vec<u8>, field: u32, value: u64) {
    // A zero value is omitted, which is protobuf's own rule for a non-repeated
    // field and keeps the encoder from writing bytes the decoder then has to
    // read back.
    if value == 0 {
        return;
    }
    write_varint(out, u64::from(field) << 3);
    write_varint(out, value);
}

fn write_bytes_field(out: &mut Vec<u8>, field: u32, value: &[u8]) {
    write_varint(out, (u64::from(field) << 3) | 2);
    write_varint(out, value.len() as u64);
    out.extend_from_slice(value);
}

fn write_string_field(out: &mut Vec<u8>, field: u32, value: &str) {
    write_bytes_field(out, field, value.as_bytes());
}

/// Base64, so the response body can carry the card JSON Feishu expects.
///
/// Standard alphabet with padding: the SDKs that read this use the same.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        let triple = (u32::from(first) << 16) | (u32::from(second) << 8) | u32::from(third);
        out.push(ALPHABET[(triple >> 18) as usize & 0x3F] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 0x3F] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3F] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3F] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_through_the_wire_format() {
        let frame = Frame {
            seq_id: 1,
            log_id: 42,
            service: 7,
            method: METHOD_DATA,
            headers: vec![(HEADER_TYPE.to_string(), "event".to_string())],
            payload_encoding: "json".into(),
            payload_type: String::new(),
            payload: br#"{"hello":"world"}"#.to_vec(),
            log_id_new: String::new(),
        };
        let decoded = Frame::decode(&frame.encode()).expect("a frame it just wrote");
        assert_eq!(decoded.seq_id, 1);
        assert_eq!(decoded.log_id, 42);
        assert_eq!(decoded.service, 7);
        assert_eq!(decoded.method, METHOD_DATA);
        assert_eq!(decoded.kind(), Some("event"));
        assert_eq!(decoded.payload_text(), Some(r#"{"hello":"world"}"#));
    }

    #[test]
    fn a_length_prefix_is_read_when_present_and_ignored_when_not() {
        let frame = Frame {
            seq_id: 5,
            method: METHOD_DATA,
            headers: vec![(HEADER_TYPE.to_string(), "event".to_string())],
            payload: b"{}".to_vec(),
            ..Frame::default()
        };
        let encoded = frame.encode();
        // Straight off a stream: the prefix is the first varint.
        assert_eq!(Frame::decode(&encoded).unwrap().seq_id, 5);
        // Already split by a caller that read the length itself.
        let (_, prefix) = read_varint(&encoded).unwrap();
        assert_eq!(Frame::decode(&encoded[prefix..]).unwrap().seq_id, 5);
    }

    #[test]
    fn a_ping_is_a_control_frame_named_by_its_header() {
        let ping = Frame::ping(7);
        assert_eq!(ping.kind(), Some("ping"));
        assert_eq!(ping.method, METHOD_CONTROL);
        assert_eq!(ping.service, 7);
    }

    #[test]
    fn a_response_carries_a_code_and_base64_data() {
        let request = Frame {
            seq_id: 9,
            log_id: 3,
            service: 1,
            method: METHOD_DATA,
            ..Frame::default()
        };
        let response = Frame::response(&request, 200, Some(r#"{"toast":{"type":"success"}}"#));
        assert_eq!(response.seq_id, 9, "the answer is matched to its request");
        assert_eq!(response.log_id, 3);
        let body: serde_json::Value =
            serde_json::from_slice(&response.payload).expect("a JSON body");
        assert_eq!(body["code"], 200);
        // The card response is base64 of the JSON the client reads.
        let data = body["data"].as_str().expect("a data string");
        assert_eq!(
            String::from_utf8_lossy(&decode_base64(data).unwrap()),
            r#"{"toast":{"type":"success"}}"#
        );
    }

    #[test]
    fn a_response_with_no_data_sends_a_null() {
        let response = Frame::response(&Frame::default(), 500, None);
        let body: serde_json::Value = serde_json::from_slice(&response.payload).unwrap();
        assert_eq!(body["code"], 500);
        assert!(body["data"].is_null());
    }

    #[test]
    fn an_unknown_field_is_skipped_rather_than_failing_the_frame() {
        // Feishu adds fields; a frame that fails to parse is a dropped message
        // with nothing in the log to explain it.
        let encoded = Frame {
            seq_id: 4,
            method: METHOD_DATA,
            headers: vec![(HEADER_TYPE.to_string(), "event".to_string())],
            ..Frame::default()
        }
        .encode();
        // Strip the prefix, then append field 15 as a length-delimited string.
        let (_, prefix) = read_varint(&encoded).expect("a prefix");
        let mut body = encoded[prefix..].to_vec();
        write_string_field(&mut body, 15, "something new");
        let decoded = Frame::decode_body(&body).expect("it still parses");
        assert_eq!(decoded.seq_id, 4);
        assert_eq!(decoded.kind(), Some("event"));
    }

    #[test]
    fn a_truncated_frame_is_refused_rather_than_read_past_the_end() {
        let encoded = Frame {
            seq_id: 1,
            payload: b"0123456789".to_vec(),
            ..Frame::default()
        }
        .encode();
        assert!(Frame::decode(&encoded[..encoded.len() - 4]).is_none());
        // A length-delimited field claiming more than the buffer holds.
        let mut lying = vec![0x08, 0x01];
        lying.extend_from_slice(&[0x42, 0xFF, 0x01]);
        assert!(Frame::decode(&lying).is_none());
    }

    #[test]
    fn an_encoding_this_adapter_does_not_know_does_not_produce_mangled_text() {
        let frame = Frame {
            payload: b"not text".to_vec(),
            payload_encoding: "gzip".into(),
            ..Frame::default()
        };
        assert_eq!(frame.payload_text(), None);
    }

    fn decode_base64(text: &str) -> Option<Vec<u8>> {
        const ALPHABET: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = Vec::new();
        let mut buffer = 0u32;
        let mut bits = 0u32;
        for character in text.chars() {
            if character == '=' {
                break;
            }
            let index = ALPHABET
                .iter()
                .position(|byte| *byte as char == character)?;
            buffer = (buffer << 6) | index as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buffer >> bits) as u8);
            }
        }
        Some(out)
    }
}

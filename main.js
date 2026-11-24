const RELEASES_API = "https://api.github.com/repos/BerryUIKI/CowBot/releases";
const RELEASES_PAGE = "https://github.com/BerryUIKI/CowBot/releases";

const ASSET_PATTERNS = {
  windows: { exe: /x64-setup\.exe$|x86_64.*\.exe$/i, msi: /\.msi$/i },
  macos: { dmg: /\.dmg$/i },
  linux: { appimage: /\.AppImage$/i, deb: /_amd64\.deb$|\.deb$/i },
};

const PLATFORM_LABELS = {
  windows: "Windows",
  macos: "macOS",
  linux: "Linux",
};

function detectPlatform() {
  const ua = navigator.userAgent;
  const platform = navigator.userAgentData?.platform || navigator.platform || "";
  if (/Mac/i.test(platform)) return "macos";
  if (/Win/i.test(platform)) return "windows";
  if (/Linux|X11/i.test(platform) && !/Android/i.test(ua)) return "linux";
  return null;
}

function findAsset(assets, patterns) {
  for (const pattern of patterns) {
    const match = assets.find((asset) => pattern.test(asset.name));
    if (match) return match;
  }
  return null;
}

function updateDownloadLinks(release) {
  for (const [os, patterns] of Object.entries(ASSET_PATTERNS)) {
    for (const [key, pattern] of Object.entries(patterns)) {
      const asset = findAsset(release.assets, [pattern]);
      if (!asset) continue;
      document
        .querySelectorAll(`[data-os-link="${os}"][data-asset="${key}"]`)
        .forEach((link) => {
          link.href = asset.browser_download_url;
        });
    }
  }

  document.querySelectorAll("[data-release-version]").forEach((node) => {
    node.textContent = `${release.tag_name} · ${new Date(
      release.published_at
    ).toLocaleDateString(undefined, { year: "numeric", month: "short" })}`;
  });

  const platform = detectPlatform();
  const heroLink = document.getElementById("hero-download");
  if (heroLink && platform) {
    const asset = findAsset(
      release.assets,
      Object.values(ASSET_PATTERNS[platform])
    );
    if (asset) heroLink.href = asset.browser_download_url;
  }
}

function setHeroLabel(platform) {
  const heroLink = document.getElementById("hero-download");
  if (!heroLink || !platform) return;
  heroLink.textContent = `Download for ${PLATFORM_LABELS[platform]}`;
}

async function loadLatestRelease() {
  try {
    const response = await fetch(`${RELEASES_API}/latest`, {
      headers: { Accept: "application/vnd.github+json" },
    });
    if (!response.ok) throw new Error(`GitHub returned ${response.status}`);
    const release = await response.json();
    if (Array.isArray(release.assets)) updateDownloadLinks(release);
  } catch (error) {
    // Keep the releases-page fallbacks already in the markup.
    console.info("Using release page links:", error.message);
  }
}

function wireApproval() {
  const approval = document.getElementById("approval");
  const approve = document.getElementById("approve");
  const reject = document.getElementById("reject");
  const result = document.getElementById("approval-result");
  const reviewer = document.getElementById("reviewer-msg");
  if (!approval || !approve || !reject || !result || !reviewer) return;

  let decided = false;

  function decide(kind) {
    if (decided) return;
    decided = true;
    result.hidden = false;
    result.classList.toggle("is-rejected", kind === "rejected");
    if (kind === "approved") {
      result.textContent = "Approved — running cargo test --workspace";
      reviewer.classList.remove("is-hidden");
      reviewer.classList.add("is-new");
      approval.querySelector(".approval-actions").hidden = true;
    } else {
      result.textContent = "Rejected — the coder will revise the plan";
      approval.querySelector(".approval-actions").hidden = true;
      reviewer.classList.add("is-hidden");
    }
  }

  approve.addEventListener("click", () => decide("approved"));
  reject.addEventListener("click", () => decide("rejected"));
}

function wireCopy() {
  document.querySelectorAll("[data-copy]").forEach((button) => {
    button.addEventListener("click", async () => {
      const source = document.getElementById(button.dataset.copy);
      if (!source) return;
      try {
        await navigator.clipboard.writeText(source.textContent);
        button.textContent = "Copied";
        setTimeout(() => {
          button.textContent = "Copy";
        }, 1600);
      } catch {
        button.textContent = "Press Ctrl+C";
      }
    });
  });
}

function wireReveals() {
  const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const targets = document.querySelectorAll("[data-reveal]");
  if (!targets.length) return;
  if (reduce || !("IntersectionObserver" in window)) {
    targets.forEach((node) => node.classList.add("is-visible"));
    return;
  }
  document.documentElement.classList.add("reveal-ready");
  const pending = new Set(targets);
  const observer = new IntersectionObserver(
    () => {
      const limit = window.innerHeight * 0.96;
      pending.forEach((node) => {
        const { top, bottom } = node.getBoundingClientRect();
        if (bottom > 0 && top < limit) {
          node.classList.add("is-visible");
          pending.delete(node);
          observer.unobserve(node);
        }
      });
    },
    { rootMargin: "0px 0px -8% 0px", threshold: 0 }
  );
  targets.forEach((node) => observer.observe(node));
  setTimeout(() => {
    if (!document.querySelector("[data-reveal].is-visible")) {
      document.documentElement.classList.remove("reveal-ready");
      targets.forEach((node) => observer.unobserve(node));
    }
  }, 1500);
}

function wirePolicyTabs() {
  const tabs = [...document.querySelectorAll(".policy-tab")];
  const panels = [...document.querySelectorAll(".policy-panel")];
  if (!tabs.length || !panels.length) return;

  function select(tab) {
    tabs.forEach((node) => {
      const active = node === tab;
      node.setAttribute("aria-selected", String(active));
      node.tabIndex = active ? 0 : -1;
    });
    panels.forEach((panel) => {
      panel.hidden = panel.dataset.panel !== tab.dataset.policy;
    });
  }

  tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (!step) return;
      event.preventDefault();
      const next = tabs[(index + step + tabs.length) % tabs.length];
      select(next);
      next.focus();
    });
  });
}

function wireThemeToggle() {
  const button = document.getElementById("theme-toggle");
  const label = button?.querySelector(".theme-label");
  if (!button) return;
  const media = window.matchMedia("(prefers-color-scheme: dark)");

  const current = () =>
    document.documentElement.dataset.theme || (media.matches ? "dark" : "light");

  function sync() {
    const dark = current() === "dark";
    button.setAttribute("aria-pressed", String(dark));
    if (label) label.textContent = dark ? "Light" : "Dark";
    button.title = dark ? "Switch to light mode" : "Switch to dark mode";
  }

  button.addEventListener("click", () => {
    document.documentElement.dataset.theme = current() === "dark" ? "light" : "dark";
    sync();
  });

  media.addEventListener("change", () => {
    if (!document.documentElement.dataset.theme) sync();
  });

  sync();
}

function wireDemoParallax() {
  const demo = document.querySelector(".demo");
  if (!demo || window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
  if (window.matchMedia("(pointer: coarse)").matches) return;
  let frame = 0;
  demo.addEventListener("pointermove", (event) => {
    if (frame) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      const rect = demo.getBoundingClientRect();
      const x = (event.clientX - rect.left) / rect.width - 0.5;
      const y = (event.clientY - rect.top) / rect.height - 0.5;
      demo.style.transform = `perspective(900px) rotateX(${(-y * 2).toFixed(2)}deg) rotateY(${(x * 2.5).toFixed(2)}deg)`;
    });
  });
  demo.addEventListener("pointerleave", () => {
    demo.style.transform = "";
  });
}

const platform = detectPlatform();
setHeroLabel(platform);
wireApproval();
wireCopy();
wireReveals();
wirePolicyTabs();
wireThemeToggle();
wireDemoParallax();
loadLatestRelease();

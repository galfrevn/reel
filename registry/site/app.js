/* reel templates — gallery behavior. Zero dependencies. */
(() => {
  "use strict";

  const $ = (sel, el = document) => el.querySelector(sel);
  const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];

  const DATA = JSON.parse($("#reel-data").textContent);
  const bySlug = new Map(DATA.map((d) => [d.slug, d]));
  const cards = $$(".card");
  const packs = $$(".pack");

  /* ---------- lazy video: load near viewport, play only while visible ---------- */

  const loadIO = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        const v = e.target;
        if (v.dataset.src) {
          v.src = v.dataset.src;
          delete v.dataset.src;
        }
        loadIO.unobserve(v);
      }
    },
    { rootMargin: "400px" }
  );

  const playIO = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        const v = e.target;
        if (e.isIntersecting) v.play?.().catch(() => {});
        else v.pause?.();
      }
    },
    { threshold: 0.25 }
  );

  for (const v of $$(".card video")) {
    loadIO.observe(v);
    playIO.observe(v);
  }

  /* ---------- entrance reveal ---------- */

  const t0 = performance.now();
  const inIO = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        // Stagger only the initial viewport — during scrolling, reveal at once.
        if (performance.now() - t0 > 1200) e.target.style.setProperty("--i", 0);
        e.target.classList.add("in");
        inIO.unobserve(e.target);
      }
    },
    { rootMargin: "150px 0px -5% 0px" }
  );
  cards.forEach((c) => inIO.observe(c));

  /* ---------- pointer spotlight ---------- */

  if (matchMedia("(hover: hover) and (prefers-reduced-motion: no-preference)").matches) {
    document.addEventListener("pointermove", (e) => {
      const card = e.target.closest?.(".card");
      if (!card) return;
      const r = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${e.clientX - r.left}px`);
      card.style.setProperty("--my", `${e.clientY - r.top}px`);
    });
  }

  /* ---------- filtering: query AND kind AND (any active tag) ---------- */

  const q = $("#q");
  const chipsEl = $("#chips");
  const empty = $("#empty");
  const activeTags = new Set();
  let activeKind = "";

  // Build tag chips from the cards, most-used first.
  const tagCount = new Map();
  for (const c of cards)
    for (const t of (c.dataset.tags || "").split(" ").filter(Boolean))
      tagCount.set(t, (tagCount.get(t) || 0) + 1);

  const VISIBLE_CHIPS = 9;
  const sortedTags = [...tagCount].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  sortedTags.forEach(([tag, n], idx) => {
    const b = document.createElement("button");
    b.className = "chip";
    b.dataset.tag = tag;
    b.hidden = idx >= VISIBLE_CHIPS;
    b.innerHTML = `${tag}<span class="n">${n}</span>`;
    b.addEventListener("click", () => {
      b.classList.toggle("is-active");
      activeTags[b.classList.contains("is-active") ? "add" : "delete"](tag);
      apply();
    });
    chipsEl.append(b);
  });
  if (sortedTags.length > VISIBLE_CHIPS) {
    const more = document.createElement("button");
    more.className = "chip chip-more";
    more.textContent = `+${sortedTags.length - VISIBLE_CHIPS} more`;
    more.addEventListener("click", () => {
      const expand = more.textContent.startsWith("+");
      // Keep active chips visible when collapsing back.
      $$(".chip", chipsEl).forEach((c, idx) => {
        if (c === more) return;
        c.hidden = !expand && idx >= VISIBLE_CHIPS && !c.classList.contains("is-active");
      });
      more.textContent = expand ? "less" : `+${sortedTags.length - VISIBLE_CHIPS} more`;
      chipsEl.append(more); // stays at the end
    });
    chipsEl.append(more);
  }

  function matches(card) {
    if (activeKind && card.dataset.kind !== activeKind) return false;
    if (activeTags.size) {
      const tags = (card.dataset.tags || "").split(" ");
      if (![...activeTags].some((t) => tags.includes(t))) return false;
    }
    const needle = q.value.trim().toLowerCase();
    if (needle && !card.dataset.search.includes(needle)) return false;
    return true;
  }

  function apply() {
    let shown = 0;
    for (const card of cards) {
      const ok = matches(card);
      card.classList.toggle("hide", !ok);
      if (ok) shown++;
    }
    for (const pack of packs) {
      const any = $$(".card", pack).some((c) => !c.classList.contains("hide"));
      pack.classList.toggle("hide", !any);
    }
    empty.hidden = shown > 0;
    if (!shown) {
      const needle = q.value.trim();
      $("#empty-q").textContent = needle ? ` for “${needle}”` : " your filters";
    }
  }

  q.addEventListener("input", apply);

  $("#clear").addEventListener("click", () => {
    q.value = "";
    activeTags.clear();
    activeKind = "";
    $$(".chip.is-active", chipsEl).forEach((c) => c.classList.remove("is-active"));
    $$(".kind").forEach((k) => k.classList.toggle("is-active", !k.dataset.kind));
    apply();
  });

  for (const k of $$(".kind")) {
    k.addEventListener("click", () => {
      activeKind = k.dataset.kind;
      $$(".kind").forEach((x) => x.classList.toggle("is-active", x === k));
      apply();
    });
  }

  // A card's tag pills toggle the matching filter chip.
  document.addEventListener("click", (e) => {
    const pill = e.target.closest("button.tag");
    if (!pill) return;
    e.stopPropagation();
    $(`.chip[data-tag="${CSS.escape(pill.dataset.tag)}"]`, chipsEl)?.click();
    $("#rail").scrollIntoView({ block: "nearest", behavior: "smooth" });
  });

  /* ---------- keyboard ---------- */

  document.addEventListener("keydown", (e) => {
    if (e.key === "/" && !e.metaKey && !e.ctrlKey && document.activeElement !== q &&
        !/^(INPUT|TEXTAREA)$/.test(document.activeElement?.tagName)) {
      e.preventDefault();
      q.focus();
      q.select();
    } else if (e.key === "Escape" && document.activeElement === q) {
      if (q.value) { q.value = ""; apply(); }
      else q.blur();
    }
  });

  /* ---------- copy-to-install ---------- */

  async function copyText(text) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Clipboard API needs a focused secure context — fall back for the rest.
      const ta = document.createElement("textarea");
      ta.value = text;
      ta.style.cssText = "position:fixed;opacity:0";
      document.body.append(ta);
      ta.select();
      let ok = false;
      try { ok = document.execCommand("copy"); } catch { /* stays false */ }
      ta.remove();
      return ok;
    }
  }

  document.addEventListener("click", (e) => {
    const btn = e.target.closest(".install");
    if (!btn) return;
    e.stopPropagation();
    copyText(btn.dataset.cmd).then((ok) => {
      if (!ok) return;
      const code = $("code", btn);
      if (!btn.dataset.label) btn.dataset.label = code.textContent;
      code.textContent = "copied to clipboard";
      btn.classList.add("copied");
      clearTimeout(btn._t);
      btn._t = setTimeout(() => {
        code.textContent = btn.dataset.label;
        btn.classList.remove("copied");
      }, 1400);
    });
  });

  /* ---------- audio playback ---------- */

  const audio = new Audio();
  let playingCard = null;

  // Show each sound's duration up front (the WAVs are tiny).
  for (const sc of $$(".card-sound")) {
    const probe = new Audio();
    probe.preload = "metadata";
    probe.src = $(".play", sc).dataset.audio;
    probe.addEventListener("loadedmetadata", () => {
      if (isFinite(probe.duration))
        $(".dur", sc).textContent = `${probe.duration.toFixed(2)}s`;
    }, { once: true });
  }

  function stopAudio() {
    audio.pause();
    playingCard?.classList.remove("playing");
    playingCard = null;
  }
  audio.addEventListener("ended", stopAudio);
  audio.addEventListener("loadedmetadata", () => {
    if (!playingCard) return;
    const dur = $(".dur", playingCard);
    if (dur && isFinite(audio.duration))
      dur.textContent = `${audio.duration.toFixed(2)}s`;
  });

  document.addEventListener("click", (e) => {
    const btn = e.target.closest(".play");
    if (!btn) return;
    e.stopPropagation();
    const host = btn.closest(".card, .detail-media");
    if (playingCard === host && !audio.paused) return stopAudio();
    stopAudio();
    audio.src = btn.dataset.audio;
    audio.play().catch(() => {});
    playingCard = host;
    host.classList.add("playing");
  });

  /* ---------- detail dialog ---------- */

  const dlg = $("#detail");
  const media = $("#detail-media");

  const TOML_KV = /^(\s*)([A-Za-z0-9_.-]+)(\s*=\s*)(.*)$/;
  const esc = (s) =>
    s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  function tomlValue(raw) {
    let out = esc(raw)
      .replace(/&quot;/g, '"')
      .replace(/"([^"]*)"/g, (m, inner) => {
        const sw = /^#(?:[0-9a-fA-F]{3,4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(inner)
          ? `<i class="sw" style="background:${inner}"></i>` : "";
        return `<span class="t-str">"${inner}"</span>${sw}`;
      });
    out = out.replace(/\b(true|false)\b(?![^<]*<\/span>)/g, '<span class="t-bool">$1</span>');
    out = out.replace(/(^|[\s,[])(-?\d+(?:\.\d+)?)(?![^<]*<\/span>)/g, '$1<span class="t-num">$2</span>');
    return out;
  }

  function highlightToml(src) {
    return src.split("\n").map((line) => {
      const hash = line.indexOf("#");
      let comment = "";
      // Only treat # as a comment when it isn't inside a quoted color value.
      if (hash >= 0 && (line.slice(0, hash).match(/"/g) || []).length % 2 === 0) {
        comment = `<span class="t-cm">${esc(line.slice(hash))}</span>`;
        line = line.slice(0, hash);
      }
      if (/^\s*\[.*\]\s*$/.test(line))
        return `<span class="t-sec">${esc(line)}</span>` + comment;
      const kv = line.match(TOML_KV);
      if (kv)
        return `${kv[1]}<span class="t-key">${esc(kv[2])}</span>${kv[3]}${tomlValue(kv[4])}` + comment;
      return tomlValue(line) + comment;
    }).join("\n");
  }

  function openDetail(slug) {
    const d = bySlug.get(slug);
    if (!d) return;
    // The description line is the one value long enough to force x-scroll —
    // truncate it in the displayed source (the full text sits right above).
    const toml = d.toml.replace(
      /^(\s*description\s*=\s*")([^"]{37,})(")/m,
      (m, a, b, c) => a + b.slice(0, 34).trimEnd() + "…" + c
    );
    $("#detail-name").textContent = d.name;
    $("#detail-desc").textContent = d.desc;
    $("#detail-tags").innerHTML = d.tags
      .map((t) => `<span class="tag">${esc(t)}</span>`).join("");
    const install = $("#detail-install");
    install.dataset.cmd = d.cmd;
    delete install.dataset.label;
    install.classList.remove("copied");
    $("code", install).textContent = d.cmd;
    $("#detail-toml").innerHTML = highlightToml(toml);

    if (d.kind === "sound") {
      media.innerHTML = `
        <div class="sound-stage">
          <button class="play" data-audio="sounds/${d.slug}.wav" aria-label="Play ${esc(d.name)}">
            <svg class="ic-play" viewBox="0 0 16 16" width="15" height="15" fill="currentColor"><path d="M4 2.5v11l9-5.5z"/></svg>
            <svg class="ic-pause" viewBox="0 0 16 16" width="15" height="15" fill="currentColor"><path d="M4 2.5h3v11H4zm5 0h3v11H9z"/></svg>
          </button>
          <div class="eq" aria-hidden="true">${"<i></i>".repeat(24)}</div>
          <span class="dur"></span>
        </div>`;
    } else {
      media.innerHTML = `
        <video autoplay loop muted playsinline
               poster="posters/${d.slug}.png" src="previews/${d.slug}.webm"></video>`;
    }
    dlg.showModal();
    const pre = $(".toml", dlg);
    pre.scrollLeft = pre.scrollTop = 0;
    $(".detail-body", dlg).scrollTop = 0;
  }

  document.addEventListener("click", (e) => {
    const open = e.target.closest(".preview, .name");
    if (!open) return;
    const card = open.closest(".card");
    if (card) openDetail(card.dataset.slug);
  });

  function closeDetail() {
    stopAudio();
    media.innerHTML = "";
    dlg.close();
  }
  $("#detail-close").addEventListener("click", closeDetail);
  dlg.addEventListener("click", (e) => {
    if (e.target === dlg) closeDetail(); // backdrop
  });
  dlg.addEventListener("cancel", (e) => {
    e.preventDefault();
    closeDetail();
  });
})();

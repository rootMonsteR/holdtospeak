// HoldToSpeak settings window — plain JS over Tauri commands (see src/settings_ui.rs).
(function () {
  'use strict';
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args);
  const $ = (sel, root) => (root || document).querySelector(sel);
  const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

  let state = null;          // last get_state()
  let page = 'general';
  let timers = [];           // per-page intervals, cleared on navigation
  const content = $('#content');

  // ---------- utilities ----------
  function toast(msg, isErr) {
    const t = $('#toast');
    t.textContent = msg; t.classList.toggle('err', !!isErr); t.hidden = false;
    clearTimeout(toast._t); toast._t = setTimeout(() => { t.hidden = true; }, isErr ? 4200 : 1800);
  }
  async function call(cmd, args, okMsg) {
    try { const r = await invoke(cmd, args); if (okMsg) toast(okMsg); return r; }
    catch (e) { toast(String(e), true); throw e; }
  }
  function keys(combo) {
    return '<span class="keys">' + String(combo || '').split('+').map((k) => `<span class="key">${esc(k)}</span>`).join('<span class="plus">+</span>') + '</span>';
  }
  const toggle = (on, action, extra) => `<button class="toggle${on ? ' on' : ''}" data-action="${action}" ${extra || ''} role="switch" aria-checked="${!!on}"></button>`;
  const sec = (label, inner) => `<section class="sec"><div class="sec-label">${esc(label)}</div>${inner}</section>`;
  const row = (t, d, right) => `<div class="row"><div><div class="row-t">${t}</div>${d ? `<div class="row-d">${d}</div>` : ''}</div><div class="row-r">${right || ''}</div></div>`;
  const brackets = '<span class="brk tl"></span><span class="brk tr"></span><span class="brk bl"></span><span class="brk br"></span>';
  const MODE_COLORS = { raw: ['#AFB2BE', '#E8EAF4'], auto: ['#28AAFF', '#5ADCFF'], polish: ['#28DC96', '#96F06E'], email: ['#BE50FF', '#F078E1'] };
  const modeDot = (tok) => `<span class="mode-dot" style="background:${(MODE_COLORS[tok] || MODE_COLORS.auto)[1]}"></span>`;
  const human = (tok) => ({ raw: 'Raw', auto: 'Auto', polish: 'Polish', email: 'Email' }[tok] || tok);
  const MODE_DESC = {
    raw: 'Exactly what you said, plus your dictionary.',
    auto: 'Light tidy: um/uh removed, sentence casing, end punctuation.',
    polish: 'Auto, plus comma-delimited filler like “you know” removed.',
    email: 'Formal prose rewrite — needs the LLM cleanup sidecar.',
  };

  // ---------- overlay previews (real overlay geometry: 460×84) ----------
  function mix(a, b, t) { const h = (c) => c.replace('#', '').match(/../g).map((x) => parseInt(x, 16)); const A = h(a), B = h(b); return '#' + A.map((v, i) => Math.round(v + (B[i] - v) * t).toString(16).padStart(2, '0')).join(''); }
  const MONO = 'font-family="Cascadia Mono, Consolas, monospace"';
  function statusRow(c0, c1, label, level) {
    let s = `<circle cx="15" cy="7" r="2.6" fill="${c1}" fill-opacity="0.9"/><text x="23" y="10.5" ${MONO} font-size="8.5" letter-spacing="1" fill="${c1}" fill-opacity="0.85">${label}</text>`;
    const lit = Math.ceil(level * 5);
    for (let i = 0; i < 5; i++) { const h = 2 + i * 2, x = 460 - 15 - 23 + i * 5, y = 11 - h; s += `<rect x="${x}" y="${y}" width="3" height="${h}" fill="${i < lit ? mix(c0, c1, i / 4) : c0}" fill-opacity="${i < lit ? 0.85 : 0.16}"/>`; }
    return s;
  }
  function hudPreview(label) {
    let top = [], bot = [];
    for (let x = 56; x <= 444; x += 3) { const t = (x - 56) / 388; const a = 0.5 + 0.5 * Math.sin(x * 0.11) * Math.sin(x * 0.037 + 1.3); const h = Math.max(1, 16 * (0.06 + 0.94 * a * a) * (0.3 + 0.7 * t)); top.push(`${x},${(46 - h).toFixed(1)}`); bot.push(`${x},${(46 + h).toFixed(1)}`); }
    const path = `M${top.join(' L')} L${bot.reverse().join(' L')} Z`;
    let meter = ''; for (let i = 0; i < 5; i++) { const bh = 3 + i * 2, lit = i < 3; meter += `<rect x="${460 - 16 - (5 - i) * 4}" y="${74 - bh}" width="2" height="${bh}" fill="${lit ? '#31D6FF' : '#246E8C'}" fill-opacity="${lit ? 0.9 : 0.35}"/>`; }
    return `<svg viewBox="0 0 460 84" width="100%" style="display:block"><defs><pattern id="scan" width="1" height="3" patternUnits="userSpaceOnUse"><rect width="1" height="1" fill="#05080D" fill-opacity="0.55"/></pattern><linearGradient id="vp" x1="0" x2="1"><stop offset="0" stop-color="#246E8C"/><stop offset="1" stop-color="#31D6FF"/></linearGradient></defs><polygon points="10,0 450,0 460,10 460,74 450,84 10,84 0,74 0,10" fill="#080C12" fill-opacity="0.86"/><rect x="5" y="5" width="450" height="74" fill="url(#scan)"/><line x1="13" y1="2.5" x2="447" y2="2.5" stroke="#31D6FF" stroke-opacity="0.4"/><line x1="13" y1="81.5" x2="447" y2="81.5" stroke="#31D6FF" stroke-opacity="0.22"/><path d="M0,13 L13,0 M447,0 L460,13 M0,71 L13,84 M447,84 L460,71" stroke="#31D6FF" stroke-opacity="0.9" stroke-width="1.2" fill="none"/><rect x="14" y="10" width="4" height="4" fill="#FFB02E" fill-opacity="0.95"/><text x="24" y="15.5" ${MONO} font-size="8.5" letter-spacing="1" fill="#8CE1FF" fill-opacity="0.9">TRANSMITTING</text><text x="96" y="15.5" ${MONO} font-size="8.5" letter-spacing="1" fill="#246E8C">00:07</text><text x="444" y="15.5" ${MONO} font-size="8.5" letter-spacing="1" fill="#E1F5FF" text-anchor="end">${label}</text><circle cx="30" cy="46" r="12" fill="#31D6FF" fill-opacity="0.12"/><circle cx="30" cy="46" r="7.6" fill="none" stroke="#31D6FF" stroke-opacity="0.85" stroke-width="1.4"/><circle cx="30" cy="46" r="3.2" fill="#E1F5FF"/><line x1="56" y1="46.5" x2="444" y2="46.5" stroke="#246E8C" stroke-opacity="0.35"/><path d="${path}" fill="url(#vp)" fill-opacity="0.22" stroke="url(#vp)" stroke-opacity="0.9" stroke-width="1"/><text x="16" y="76" ${MONO} font-size="8.5" letter-spacing="1" fill="#246E8C">LINK 01 . SECURE</text>${meter}</svg>`;
  }
  function voltPreview(c0, c1, label) {
    const pts = []; for (let x = 24; x <= 436; x += 4) pts.push(`${x},${(52 + Math.sin(x * 0.21) * 2.2 + Math.sin(x * 0.067 + 2) * 3.5 + Math.sin(x * 0.013) * 4).toFixed(1)}`);
    const beam = 'M' + pts.join(' L');
    return `<svg viewBox="0 0 460 84" width="100%" style="display:block"><defs><filter id="g1" x="-5%" y="-50%" width="110%" height="200%"><feGaussianBlur stdDeviation="3"/></filter><filter id="g2" x="-5%" y="-50%" width="110%" height="200%"><feGaussianBlur stdDeviation="1.2"/></filter></defs><rect width="460" height="84" rx="20" fill="#080A12" fill-opacity="0.6"/>${statusRow(c0, c1, label, 0.7)}<path d="${beam}" fill="none" stroke="#1F4BFF" stroke-opacity="0.55" stroke-width="9" filter="url(#g1)"/><path d="${beam}" fill="none" stroke="#31D6FF" stroke-opacity="0.85" stroke-width="3" filter="url(#g2)"/><path d="${beam}" fill="none" stroke="#FFFFFF" stroke-opacity="0.95" stroke-width="1"/><path d="M140,50 L148,38 L146,41 L156,30 M300,54 L294,66 L297,62 L288,72 M372,51 L380,40 L378,44 L386,36" fill="none" stroke="#CFF3FF" stroke-opacity="0.8" stroke-width="1"/><rect x="24" y="77" width="412" height="1.2" fill="${c1}" fill-opacity="0.6"/></svg>`;
  }
  function wavePreview(c0, c1, label) {
    const curve = (amp, f, ph) => { const p = []; for (let x = 12; x <= 448; x += 4) p.push(`${x},${(50 + amp * Math.sin(x * f + ph) * Math.sin((x - 12) / 436 * Math.PI)).toFixed(1)}`); return 'M' + p.join(' L'); };
    let grat = ''; for (const y of [28, 39, 50, 61, 72]) grat += `<line x1="12" y1="${y}" x2="448" y2="${y}" stroke="${c0}" stroke-opacity="${y === 50 ? 0.25 : 0.08}"/>`;
    return `<svg viewBox="0 0 460 84" width="100%" style="display:block"><defs><linearGradient id="wg" x1="0" x2="1"><stop offset="0" stop-color="${c0}"/><stop offset="1" stop-color="${c1}"/></linearGradient><filter id="wb" x="-5%" y="-50%" width="110%" height="200%"><feGaussianBlur stdDeviation="2"/></filter></defs><rect width="460" height="84" rx="18" fill="#0D0E16" fill-opacity="0.68"/>${statusRow(c0, c1, label, 0.5)}${grat}<path d="${curve(22, 0.045, 0.8)}" fill="none" stroke="url(#wg)" stroke-opacity="0.35" stroke-width="4" filter="url(#wb)"/><path d="${curve(16, 0.062, 2.1)}" fill="none" stroke="url(#wg)" stroke-opacity="0.45" stroke-width="1.2"/><path d="${curve(10, 0.09, 0.2)}" fill="none" stroke="url(#wg)" stroke-opacity="0.35" stroke-width="1.2"/><path d="${curve(22, 0.045, 0.8)}" fill="none" stroke="url(#wg)" stroke-opacity="0.95" stroke-width="1.6"/></svg>`;
  }
  function barsPreview(c0, c1, label) {
    let bars = ''; const n = 32, pad = 15, gap = 2, bw = Math.floor((460 - 2 * pad - gap * (n - 1)) / n);
    for (let i = 0; i < n; i++) { const t = i / (n - 1); const lvl = Math.max(0.05, Math.exp(-Math.pow((t - 0.3) / 0.25, 2)) * 0.9 + Math.exp(-Math.pow((t - 0.75) / 0.12, 2)) * 0.35 + 0.08 * Math.sin(i * 1.7)); const h = Math.max(2, Math.round(lvl * 56)); bars += `<rect x="${pad + i * (bw + gap)}" y="${78 - h}" width="${bw}" height="${h}" rx="1" fill="${mix(c0, c1, t)}" fill-opacity="0.9"/>`; }
    return `<svg viewBox="0 0 460 84" width="100%" style="display:block"><rect width="460" height="84" rx="16" fill="#12121A" fill-opacity="0.8"/>${statusRow(c0, c1, label, 0.8)}${bars}</svg>`;
  }
  const PREVIEWS = { hud: (c0, c1, l) => hudPreview(l), volt: voltPreview, wave: wavePreview, bars: barsPreview };
  const THEME_DESC = { hud: 'Tactical comms: voiceprint, timecode, mode callout and signal meter.', volt: 'Electric beam with lightning forks that ride your voice.', wave: 'Flowing layered waveform, tinted by the active mode.', bars: 'Frequency spectrum — the classic visualizer look.' };

  // ---------- pages ----------
  const pages = {
    general(s) {
      const hk = s.hotkeys;
      const mods = hk.ptt.split('+');
      const modBtn = (m) => `<button class="mod${mods.includes(m) ? ' on' : ''}" data-action="ptt-mod" data-mod="${m}">${m}</button>`;
      const chordRow = (label, desc, field, value) => row(esc(label), esc(desc),
        `<input class="field mono" data-chord="${field}" value="${esc(value || '')}" placeholder="off" size="14" spellcheck="false" title="Click, then press the keys — or type e.g. Ctrl+Alt+M. Empty = off.">` +
        `<button class="btn ghost" data-action="chord-apply" data-chord="${field}">Apply</button>`);
      return `
        <div class="page-h"><h1>General</h1><p>Hotkeys, cleanup mode and startup.</p></div>
        ${sec('Push to talk', `<div class="card" style="padding:14px 20px;display:flex;align-items:center;justify-content:space-between;gap:20px;">${brackets}
          <div style="display:flex;align-items:center;gap:20px;">${keys(hk.ptt).replace(/class="key"/g, 'class="key big"')}
            <div><div style="font-size:14px;font-weight:500;">Hold to dictate</div><div style="font-size:12px;color:var(--text-3);margin-top:2px;">Modifier keys only. Hold, speak, release — the text lands at your cursor.</div></div></div>
          <div class="mods" title="Toggle modifiers; applies immediately">${['Ctrl', 'Alt', 'Shift', 'Win'].map(modBtn).join('')}</div></div>`)}
        ${sec('Shortcuts', `<div class="card">
          ${chordRow('Cycle cleanup mode', 'Raw → Auto → Polish, shown in the overlay', 'cycle_mode', hk.cycle_mode)}
          ${chordRow('Cycle overlay theme', 'HUD → Volt → Wave → Bars, applies live', 'cycle_style', hk.cycle_style)}
          ${chordRow('Quit HoldToSpeak', '', 'quit', hk.quit)}
        </div><div class="hint">Changes apply immediately and are written to <span class="mono">hotkeys.toml</span>. A shortcut needs a modifier plus one main key (letter, digit, F1–F12, Space, Enter, Tab).</div>`)}
        ${sec('Cleanup mode at startup', `<div class="grid3">${s.modes.filter((m) => m.available).map((m) => `<button class="opt${s.startup_mode === m.token ? ' sel' : ''}" data-action="set-mode" data-index="${m.index}"><span class="radio"></span><div class="opt-t">${modeDot(m.token)}${human(m.token)}</div><div class="opt-d">${esc(MODE_DESC[m.token] || '')}</div></button>`).join('')}</div>
          <div class="hint"><svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"><rect x="1.8" y="3" width="12.4" height="10" rx="1.5"/><path d="M4.5 6.3l2 1.7-2 1.7M8.5 9.7h3"/></svg>Terminals and code editors always get Raw, whatever you pick here — commands are never reworded. Also applies right now (live mode: <b>${esc(human(s.modes[s.mode]?.token))}</b>).</div>`)}
        ${sec('Startup', `<div class="card">${row('Start HoldToSpeak when I sign in to Windows', 'Adds a per-user Run entry. Nothing runs elevated.', toggle(s.autostart, 'set-autostart'))}</div>`)}`;
    },

    overlay(s) {
      const tok = s.modes[s.mode]?.token || 'auto';
      const [c0, c1] = MODE_COLORS[tok] || MODE_COLORS.auto;
      const label = (s.modes[s.mode]?.label || 'AUTO');
      return `
        <div class="page-h"><h1>Overlay</h1><p>The panel shown while you hold push-to-talk.</p></div>
        <div class="card">${row('Show the overlay while I hold push-to-talk', 'A click-through panel at the bottom of the active screen. Hidden the rest of the time.', toggle(s.overlay, 'set-overlay'))}</div>
        ${sec('Theme', `<div class="grid2">${s.styles.map((st) => `<button class="opt theme${s.style === st.index ? ' sel' : ''}" data-action="set-style" data-index="${st.index}" style="padding:10px 10px 12px;gap:8px;">
            <div style="border-radius:4px;overflow:hidden;background:#1A1F2B;">${PREVIEWS[st.token](c0, c1, label)}</div>
            <div class="opt-t">${esc(st.label)}${st.token === 'hud' ? ' <span class="pill pill-cyan" style="height:18px;font-size:10px;">Default</span>' : ''}<span class="radio"></span></div>
            <div class="opt-d">${esc(THEME_DESC[st.token] || '')}</div></button>`).join('')}</div>`)}
        <div class="card">
          ${row('Position', 'Bottom centre of the screen, 90 px above the edge.', '')}
          ${row('Preview', 'Shows the overlay for 4 seconds with your live microphone — nothing is dictated.', `<button class="btn" data-action="preview-overlay" ${s.overlay ? '' : 'disabled'}><svg width="12" height="12" viewBox="0 0 16 16" fill="currentColor"><path d="M4 2.5v11l9-5.5z"/></svg> Preview overlay</button>`)}
        </div>`;
    },

    microphone(s) {
      const v = s.silence_rms;
      return `
        <div class="page-h"><h1>Microphone</h1><p>Input device, level and silence threshold.</p></div>
        ${sec('Input device', `<div class="card">${row('Microphone', 'Capture runs all the time so the first word is never clipped. Follows the Windows default input device; change it in Windows Sound settings.', `<span class="pill pill-grey mono">${esc(s.mic_name)}</span>`)}</div>`)}
        ${sec('Level', `<div class="card" style="padding:16px 16px 14px;display:flex;flex-direction:column;gap:10px;">
          <div class="meter" id="meter">${'<span></span>'.repeat(28)}<span class="gate" id="gate" style="left:18%"></span></div>
          <div style="display:flex;justify-content:space-between;font-size:11.5px;color:var(--text-3);"><span>Speak normally — the bar should move with every syllable. The amber line is the silence threshold.</span><span class="mono" id="level-txt">—</span></div></div>`)}
        ${sec('Silence threshold', `<div class="card" style="padding:16px 16px 14px;display:flex;flex-direction:column;gap:10px;">
          <div style="display:flex;align-items:center;gap:16px;"><input class="slider" id="rms" type="range" min="0" max="1000" step="1" value="${rmsToSlider(v)}"><span class="mono" id="rms-val" style="min-width:92px;text-align:right;">${v.toFixed(4)} RMS</span></div>
          <div style="display:flex;justify-content:space-between;font-size:11.5px;color:var(--text-4);"><span>0.0005 · very quiet mic</span><span>0.05 · noisy room</span></div>
          <div style="font-size:12px;color:var(--text-3);">Audio below this level is treated as silence and never sent to the recognizer, so a stray key press can’t make the model invent words. Applies live; saved to <span class="mono">settings.toml</span>.</div></div>`)}
        ${sec('Look-back', `<div class="card">${row('Audio kept from before the key press', 'So the first word of every utterance is caught.', '<span class="pill pill-grey">400 ms</span>')}</div>`)}`;
    },

    dictionary(s) {
      return `
        <div class="page-h"><h1>Dictionary &amp; apps</h1><p>Words the recognizer should learn, and apps that must stay verbatim.</p></div>
        ${sec('Dictionary', `<div class="card"><div id="dict-table"><div class="row"><div class="row-d">Loading…</div></div></div>
          <div style="display:flex;align-items:center;gap:10px;padding:12px 14px;border-top:1px solid var(--line);">
            <input class="field" id="d-heard" style="flex:1" placeholder="What it heard…" spellcheck="false"><span style="color:var(--text-4)">→</span>
            <input class="field" id="d-meant" style="flex:1" placeholder="What you meant…" spellcheck="false"><button class="btn primary" data-action="dict-add">Add</button></div></div>
          <div class="hint split"><span>Plain text in <span class="mono">${esc(s.paths.dictionary)}</span>. Adding teaches the running recognizer immediately; a removal takes effect at the next start.</span><button class="link" data-action="open-path" data-which="dictionary">Show file</button></div>`)}
        ${sec('Always verbatim', `<div class="card" style="padding:14px 16px;display:flex;flex-direction:column;gap:12px;">
          <div style="font-size:12.5px;color:var(--text-2);">These apps get <b style="color:var(--text);font-weight:500;">Raw</b> automatically, so commands and identifiers are never reworded.</div>
          <div style="display:flex;flex-wrap:wrap;gap:8px;">${['Windows Terminal', 'cmd', 'PowerShell', 'Alacritty', 'WezTerm', 'Ghostty', 'mintty', 'PuTTY', 'VS Code', 'Cursor', 'Visual Studio', 'JetBrains IDEs', 'Zed', 'Sublime Text'].map((a) => `<span class="chip">${a}</span>`).join('')}</div></div>`)}`;
    },

    privacy(s) {
      return `
        <div class="page-h"><h1>Privacy</h1><p>Built to be checked, not trusted.</p></div>
        <div class="card" style="padding:14px 22px;display:flex;align-items:center;gap:24px;">${brackets}
          <div style="display:flex;align-items:baseline;gap:12px;"><span class="mono" style="font-size:38px;font-weight:500;line-height:1;color:var(--accent);">0</span><span style="font-size:15px;color:var(--text);">network code paths</span></div>
          <div style="flex:1;font-size:12px;color:var(--text-3);">There is nothing to count: the only outbound HTTP in the whole program is the one-time speech-model download, verified against a pinned SHA-256. No server, no account, no telemetry — and recordings are deleted the moment they are transcribed.</div></div>
        ${sec('Rules', `<div class="card">
          ${row('Refuse password fields', 'The focused control is checked through UI Automation; credential fields never receive text.', '<span class="pill pill-cyan">Always on</span>')}
          ${row('Telemetry, crash reports, analytics', 'There is no code path for any of these.', '<span class="pill pill-grey">None exist</span>')}
          ${row('Screenshots, reading your documents', 'Only the focused app’s name and whether the field is a password field are read — enough to choose how to insert text.', '<span class="pill pill-grey">Never</span>')}</div>`)}
        ${sec('Prove it — firewall rule', `<div class="card" style="padding:12px 14px;display:flex;flex-direction:column;gap:10px;">
          <div class="code" id="fw">New-NetFirewallRule -DisplayName "Block HoldToSpeak outbound" -Direction Outbound -Program "$env:LOCALAPPDATA\\HoldToSpeak\\HoldToSpeak.exe" -Action Block</div>
          <div style="display:flex;align-items:center;justify-content:space-between;"><span style="font-size:12px;color:var(--text-3);">Paste into an admin PowerShell after the model is installed. Dictation keeps working with the rule in place.</span><button class="btn" data-action="copy-fw">Copy</button></div></div>`)}
        ${sec('Your data', `<div class="card">
          ${row('Settings, hotkeys, dictionary', `<span class="mono">${esc(s.paths.config_dir)}</span> — plain text you can read and edit`, '<button class="link" data-action="open-path" data-which="config">Open folder</button>')}
          ${row('Speech model', `<span class="mono">${esc(s.paths.data_dir)}\\models</span>`, '<button class="link" data-action="open-path" data-which="model">Open folder</button>')}</div>`)}`;
    },

    model(s) {
      const m = s.model;
      return `
        <div class="page-h"><h1>Speech model</h1><p>What recognizes your voice, and where it lives.</p></div>
        <div class="card" style="padding:16px 18px;display:flex;flex-direction:column;gap:14px;">
          <div style="display:flex;align-items:center;justify-content:space-between;gap:16px;">
            <div><div style="font-size:15px;font-weight:500;">${esc(m.name)}</div><div style="font-size:12px;color:var(--text-3);margin-top:1px;">NVIDIA NeMo model, ONNX export by the sherpa-onnx project · CC-BY-4.0</div></div>
            ${m.installed ? '<span class="pill pill-green">Installed</span>' : '<span class="pill pill-amber">Not found</span>'}</div>
          <div class="grid4">
            <div class="tile"><div class="tile-k">Size</div><div class="tile-v">${esc(m.human)}</div></div>
            <div class="tile"><div class="tile-k">Runs on</div><div class="tile-v">CPU</div><div class="tile-s">ONNX Runtime · int8</div></div>
            <div class="tile"><div class="tile-k">Engine</div><div class="tile-v">${s.engine === 'native' ? 'Native' : 'Python sidecar'}</div><div class="tile-s">${s.llm ? 'with LLM cleanup' : 'no Python, no LLM'}</div></div>
            <div class="tile"><div class="tile-k">Checksum</div><div class="tile-v mono" style="font-size:12px;">${esc(m.sha256.slice(0, 6))}…${esc(m.sha256.slice(-4))}</div><div class="tile-s">pinned SHA-256</div></div></div>
          <div style="display:flex;align-items:center;justify-content:space-between;gap:12px;"><span class="mono" style="font-size:12px;color:var(--text-3);overflow:hidden;text-overflow:ellipsis;white-space:nowrap;">${esc(m.dir)}</span><button class="btn" data-action="open-path" data-which="model">Open folder</button></div></div>
        ${sec('Engine', `<div class="card">${row(s.engine === 'native' ? 'Native (recommended)' : 'Python sidecar (dev)', s.engine === 'native' ? 'The bundled Rust recognizer. Deterministic cleanup that can only ever delete filler.' : 'Adds LLM cleanup (Polish / Email). Still fully offline.', `<span class="pill pill-grey">sidecar = ${esc(s.engine)}</span>`)}</div>
          <div class="hint">To switch engines, set <span class="mono">sidecar</span> in <span class="mono">settings.toml</span> and restart HoldToSpeak.</div>`)}
        ${sec('Download', `<div class="card">${row('First-run download', 'The model is the only thing HoldToSpeak ever fetches — once, verified before use.', m.installed ? '<span class="pill pill-grey">Done</span>' : '<span class="pill pill-amber">Pending</span>')}</div>`)}`;
    },

    diagnostics(s) {
      return `
        <div class="page-h"><h1>Diagnostics</h1><p>What is running, and how the last dictations went.</p></div>
        <div class="grid4">
          <div class="tile"><div class="tile-k">Hotkeys</div><div class="tile-v"><span class="dot dot-green"></span>Active</div><div class="tile-s">Hold ${esc(s.hotkeys.ptt)}</div></div>
          <div class="tile"><div class="tile-k">Speech engine</div><div class="tile-v"><span class="dot dot-green"></span>Ready</div><div class="tile-s">${esc(s.engine)} · CPU${s.llm ? ' · LLM' : ''}</div></div>
          <div class="tile"><div class="tile-k">Microphone</div><div class="tile-v"><span class="dot ${s.listening ? 'dot-amber' : 'dot-green'}"></span>${s.listening ? 'Listening' : 'Capturing'}</div><div class="tile-s" title="${esc(s.mic_name)}">${esc(s.mic_name)}</div></div>
          <div class="tile"><div class="tile-k">Overlay</div><div class="tile-v"><span class="dot ${s.overlay ? 'dot-cyan' : 'dot-grey'}"></span>${esc(s.styles.find((x) => x.index === s.style)?.label || '')}</div><div class="tile-s">${s.overlay ? 'While holding ' + esc(s.hotkeys.ptt) : 'Off'}</div></div></div>
        ${sec('Recent dictations', `<div class="card"><div id="recent"><div class="row"><div class="row-d">Loading…</div></div></div></div>
          <div class="hint" id="counters"></div>`)}`;
    },

    about(s) {
      return `
        <div class="page-h"><h1>About</h1><p>Version, licenses and help.</p></div>
        <div class="card" style="padding:22px 24px;display:flex;align-items:center;gap:20px;">${brackets}
          <img src="icon.png" alt="" width="64" height="64" style="border-radius:14px;">
          <div style="flex:1;"><div style="display:flex;align-items:baseline;gap:10px;"><span style="font-size:20px;font-weight:600;">HoldToSpeak</span><span class="mono" style="font-size:12px;color:var(--text-3);">${esc(s.version)} · x64</span></div>
            <div style="font-size:13px;color:var(--text-2);margin-top:2px;">Hold two keys, talk, it types. Local, private push-to-talk dictation for Windows.</div></div>
          <button class="btn" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak/releases/latest">Check for updates</button></div>
        <div class="hint" style="margin-top:-10px;">Opens the releases page in your browser — the app never checks in the background.</div>
        ${sec('Licenses', `<div class="card">
          ${row('HoldToSpeak', 'MIT — free for any use, source on GitHub', '<button class="link" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak">Source ↗</button>')}
          ${row('Parakeet TDT 0.6B v2', 'NVIDIA · CC-BY-4.0 · ONNX export by sherpa-onnx (Apache-2.0)', '<button class="link" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak/blob/main/THIRD-PARTY-NOTICES.md">Attribution ↗</button>')}
          ${row('Everything else', 'Rust crates and runtimes — all permissive licenses', '<button class="link" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak/blob/main/THIRD-PARTY-NOTICES.md">Third-party notices ↗</button>')}</div>`)}
        ${sec('Help', `<div class="card">
          ${row('Report a bug', 'Opens the issue tracker.', '<button class="link" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak/issues">github.com/rootMonsteR/holdtospeak ↗</button>')}
          ${row('Privacy statement', 'What runs where, and how to verify it yourself.', '<button class="link" data-action="open-url" data-url="https://github.com/rootMonsteR/holdtospeak/blob/main/PRIVACY.md">PRIVACY.md ↗</button>')}</div>`)}`;
    },
  };

  // silence slider: log scale 0.0005 .. 0.05 over 0..1000
  const RMS_MIN = 0.0005, RMS_MAX = 0.05;
  function rmsToSlider(v) { return Math.round(1000 * Math.log(v / RMS_MIN) / Math.log(RMS_MAX / RMS_MIN)); }
  function sliderToRms(x) { return RMS_MIN * Math.pow(RMS_MAX / RMS_MIN, x / 1000); }

  // ---------- rendering ----------
  async function refreshState() { state = await invoke('get_state'); paintStatus(); return state; }
  function paintStatus() {
    if (!state) return;
    $('#st-ptt').textContent = `Hold ${state.hotkeys.ptt} to dictate`;
    $('#st-mode').textContent = `MODE · ${state.modes[state.mode]?.label || ''}${state.listening ? ' · LISTENING' : ''}`;
    $('#st-version').textContent = state.version;
  }
  function clearTimers() { timers.forEach(clearInterval); timers = []; }
  async function render(p) {
    clearTimers();
    page = pages[p] ? p : 'general';
    document.querySelectorAll('.nav-item').forEach((a) => a.classList.toggle('active', a.dataset.page === page));
    if (!state) await refreshState();
    content.innerHTML = pages[page](state);
    content.scrollTop = 0;
    if (page === 'microphone') startMeter();
    if (page === 'dictionary') loadDictionary();
    if (page === 'diagnostics') { loadRecent(); timers.push(setInterval(loadRecent, 2000)); }
    if (page === 'general') wireChordCapture();
  }
  async function rerender() { await refreshState(); render(page); }

  // ---------- page behaviours ----------
  function startMeter() {
    const spans = Array.from($('#meter').querySelectorAll('span:not(.gate)'));
    const tick = async () => {
      try {
        const l = await invoke('mic_level');
        const lit = Math.round(l.level * spans.length);
        spans.forEach((sp, i) => { sp.style.background = i < lit ? mix('#1F87FF', '#9EF0FF', i / (spans.length - 1)) : ''; });
        const gdb = 20 * Math.log10(Math.max(l.gate, 1e-6));
        $('#gate').style.left = `${Math.max(0, Math.min(100, (gdb + 60) / 60 * 100))}%`;
        const db = l.rms > 0 ? 20 * Math.log10(l.rms) : -90;
        $('#level-txt').textContent = `${db.toFixed(0)} dB · rms ${l.rms.toFixed(4)}`;
      } catch (_) { /* window closing */ }
    };
    tick(); timers.push(setInterval(tick, 100));
    const sl = $('#rms'), out = $('#rms-val');
    let t = null;
    sl.addEventListener('input', () => {
      const v = sliderToRms(Number(sl.value)); out.textContent = `${v.toFixed(4)} RMS`;
      clearTimeout(t); t = setTimeout(async () => { const applied = await call('set_silence_rms', { value: v }); state.silence_rms = applied; }, 250);
    });
  }
  async function loadDictionary() {
    const list = await invoke('dictionary_list');
    const el = $('#dict-table'); if (!el) return;
    if (!list.length) { el.innerHTML = '<div class="row"><div class="row-d">No entries yet. Add one below, or say <span class="mono">learn cube ctl =&gt; kubectl</span> in the console.</div></div>'; return; }
    el.innerHTML = `<table><tr><th style="width:42%">Heard</th><th style="width:42%">Meant</th><th></th></tr>${list.map((e) => `<tr><td>${esc(e.heard)}</td><td class="mono">${esc(e.meant)}</td><td class="r"><button class="btn ghost" data-action="dict-remove" data-heard="${esc(e.heard)}" data-meant="${esc(e.meant)}" title="Remove">✕</button></td></tr>`).join('')}</table>`;
  }
  async function loadRecent() {
    const list = await invoke('recent');
    const el = $('#recent'); if (!el) return;
    const fmt = (ms) => { const d = new Date(ms); return [d.getHours(), d.getMinutes(), d.getSeconds()].map((n) => String(n).padStart(2, '0')).join(':'); };
    const OUT = { inserted: ['Inserted', 'var(--green-text)'], refused: ['Refused · password field', 'var(--amber-text)'], blocked: ['Blocked · not inserted', 'var(--amber-text)'], silence: ['Silence', 'var(--text-4)'], 'too-short': ['Too short', 'var(--text-4)'], failed: ['Failed', 'var(--red-text)'], 'no-speech': ['No speech', 'var(--text-4)'] };
    if (!list.length) { el.innerHTML = '<div class="row"><div class="row-d">Nothing yet — hold the keys and say something.</div></div>'; }
    else el.innerHTML = `<table><tr><th>Time</th><th>Target app</th><th>Mode</th><th class="r">Words</th><th class="r">Latency</th><th>Result</th></tr>${list.map((r) => { const o = OUT[r.outcome] || [r.outcome, 'inherit']; return `<tr><td class="mono" style="color:var(--text-2)">${fmt(r.at_unix_ms)}</td><td>${esc(r.app) || '<span style="color:var(--text-4)">—</span>'}</td><td><span class="pill ${r.mode === 'raw' ? 'pill-grey' : 'pill-cyan'}" style="height:18px;font-size:10.5px;">${esc(human(r.mode))}</span></td><td class="r">${r.words || '<span style="color:var(--text-4)">—</span>'}</td><td class="r mono" style="white-space:nowrap">${r.ms ? r.ms + ' ms' : '—'}</td><td style="color:${o[1]};white-space:nowrap">${o[0]}</td></tr>`; }).join('')}</table>`;
    const s = await refreshState();
    const up = s.uptime_s, h = Math.floor(up / 3600), m = Math.floor((up % 3600) / 60);
    $('#counters').textContent = `${s.inserted} inserted of ${s.total} utterances since launch · up ${h ? h + ' h ' : ''}${m} min`;
  }
  function wireChordCapture() {
    document.querySelectorAll('input[data-chord]').forEach((inp) => {
      inp.addEventListener('keydown', (e) => {
        if (e.key === 'Tab') return;
        e.preventDefault();
        if (e.key === 'Escape') { inp.blur(); return; }
        if (e.key === 'Enter') { applyChord(inp.dataset.chord); return; }
        if (e.key === 'Backspace' || e.key === 'Delete') { inp.value = ''; return; }
        const mods = []; if (e.ctrlKey) mods.push('Ctrl'); if (e.altKey) mods.push('Alt'); if (e.shiftKey) mods.push('Shift'); if (e.metaKey) mods.push('Win');
        let k = e.key;
        if (['Control', 'Alt', 'Shift', 'Meta', 'OS'].includes(k)) { inp.value = mods.join('+') + (mods.length ? '+…' : '…'); return; }
        if (k === ' ') k = 'Space'; else if (k.length === 1) k = k.toUpperCase();
        inp.value = [...mods, k].join('+');
      });
      inp.addEventListener('focus', () => inp.classList.add('capturing'));
      inp.addEventListener('blur', () => { inp.classList.remove('capturing'); if (inp.value.endsWith('…')) inp.value = ''; });
    });
  }
  async function applyChord(which) {
    const get = (f) => { const el = document.querySelector(`input[data-chord="${f}"]`); return el ? el.value.replace(/…$/, '').trim() : ''; };
    const h = { ptt: state.hotkeys.ptt, cycle_mode: get('cycle_mode'), cycle_style: get('cycle_style'), quit: get('quit') };
    void which;
    const v = await call('set_hotkeys', { h }, 'Hotkeys updated');
    state.hotkeys = v; paintStatus();
  }
  async function applyPtt(mods) {
    const h = { ptt: mods.join('+'), cycle_mode: state.hotkeys.cycle_mode || '', cycle_style: state.hotkeys.cycle_style || '', quit: state.hotkeys.quit || '' };
    const v = await call('set_hotkeys', { h }, 'Push-to-talk updated');
    state.hotkeys = v; paintStatus(); render('general');
  }

  // ---------- actions (event delegation) ----------
  content.addEventListener('click', async (e) => {
    const el = e.target.closest('[data-action]'); if (!el) return;
    const a = el.dataset.action;
    try {
      if (a === 'set-mode') { await call('set_mode', { index: Number(el.dataset.index) }); await rerender(); }
      else if (a === 'set-style') { await call('set_style', { index: Number(el.dataset.index) }); await rerender(); }
      else if (a === 'set-overlay') { await call('set_overlay', { on: !state.overlay }); await rerender(); }
      else if (a === 'set-autostart') { await call('set_autostart', { on: !state.autostart }, !state.autostart ? 'Will start with Windows' : 'Won’t start with Windows'); await rerender(); }
      else if (a === 'preview-overlay') { await call('preview_overlay'); toast('Overlay shown for 4 seconds'); }
      else if (a === 'open-path') { await call('open_path', { which: el.dataset.which }); }
      else if (a === 'open-url') { await call('open_url', { url: el.dataset.url }); }
      else if (a === 'copy-fw') { await navigator.clipboard.writeText($('#fw').textContent); toast('Copied'); }
      else if (a === 'chord-apply') { await applyChord(el.dataset.chord); }
      else if (a === 'ptt-mod') {
        const cur = new Set(state.hotkeys.ptt.split('+')); const m = el.dataset.mod;
        if (cur.has(m)) cur.delete(m); else cur.add(m);
        if (!cur.size) { toast('Push-to-talk needs at least one modifier.', true); return; }
        await applyPtt(['Ctrl', 'Alt', 'Shift', 'Win'].filter((x) => cur.has(x)));
      }
      else if (a === 'dict-add') {
        const heard = $('#d-heard').value, meant = $('#d-meant').value;
        await call('dictionary_add', { heard, meant }, `Learned: ${heard.trim()} → ${meant.trim()}`);
        $('#d-heard').value = ''; $('#d-meant').value = ''; setTimeout(loadDictionary, 400);
      }
      else if (a === 'dict-remove') { await call('dictionary_remove', { heard: el.dataset.heard, meant: el.dataset.meant }, 'Removed — takes effect at next start'); loadDictionary(); }
    } catch (_) { /* toasted by call() */ }
  });
  content.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && (e.target.id === 'd-heard' || e.target.id === 'd-meant')) content.querySelector('[data-action="dict-add"]').click();
  });
  $('#nav').addEventListener('click', (e) => { const a = e.target.closest('.nav-item'); if (a) { e.preventDefault(); location.hash = a.dataset.page; } });
  window.addEventListener('hashchange', () => render(location.hash.replace('#', '') || 'general'));
  document.addEventListener('contextmenu', (e) => { if (!(e.target.closest('input'))) e.preventDefault(); });

  // live status in the sidebar
  setInterval(() => { refreshState().catch(() => {}); }, 2000);
  render(location.hash.replace('#', '') || 'general');
})();

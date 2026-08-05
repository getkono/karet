//! Deterministic README hero artwork.

pub(crate) const HERO: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="640" viewBox="0 0 1200 640" role="img" aria-labelledby="title description">
  <title id="title">Karet terminal code editor</title>
  <desc id="description">A stylized Karet window showing a project explorer and a side-by-side Rust diff.</desc>
  <defs>
    <linearGradient id="shell" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#111827"/>
      <stop offset="1" stop-color="#0b1020"/>
    </linearGradient>
    <filter id="shadow" x="-10%" y="-10%" width="120%" height="130%">
      <feDropShadow dx="0" dy="16" stdDeviation="18" flood-color="#020617" flood-opacity=".55"/>
    </filter>
    <clipPath id="window-clip"><rect x="45" y="45" width="1110" height="550" rx="18"/></clipPath>
    <style>
      .ui { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace; }
      .muted { fill: #64748b; }
      .text { fill: #cbd5e1; }
      .bright { fill: #f8fafc; }
      .blue { fill: #7dd3fc; }
      .purple { fill: #c4b5fd; }
      .green { fill: #86efac; }
      .red { fill: #fda4af; }
      .amber { fill: #fde68a; }
    </style>
  </defs>
  <rect width="1200" height="640" fill="#070b16"/>
  <circle cx="1050" cy="80" r="250" fill="#172554" opacity=".28"/>
  <circle cx="140" cy="610" r="300" fill="#312e81" opacity=".18"/>
  <g filter="url(#shadow)">
    <rect x="45" y="45" width="1110" height="550" rx="18" fill="url(#shell)" stroke="#334155"/>
  </g>
  <g clip-path="url(#window-clip)" class="ui">
    <rect x="45" y="45" width="1110" height="43" fill="#1e293b"/>
    <circle cx="70" cy="67" r="6" fill="#fb7185"/><circle cx="90" cy="67" r="6" fill="#facc15"/><circle cx="110" cy="67" r="6" fill="#4ade80"/>
    <text x="600" y="72" text-anchor="middle" font-size="14" class="text">karet — review · src/editor.rs</text>

    <rect x="45" y="88" width="228" height="477" fill="#0f172a"/>
    <rect x="273" y="88" width="1" height="477" fill="#334155"/>
    <text x="66" y="120" font-size="12" font-weight="700" letter-spacing="1.5" class="muted">EXPLORER</text>
    <text x="66" y="151" font-size="14" class="bright">⌄  karet</text>
    <text x="84" y="179" font-size="14" class="text">⌄  crates</text>
    <text x="102" y="207" font-size="14" class="text">⌄  karet-editor</text>
    <rect x="55" y="219" width="208" height="30" rx="5" fill="#1e3a5f"/>
    <text x="120" y="240" font-size="14" class="blue">◆  editor.rs</text>
    <text x="102" y="272" font-size="14" class="text">◆  lib.rs</text>
    <text x="84" y="300" font-size="14" class="text">›  karet-core</text>
    <text x="84" y="328" font-size="14" class="text">›  karet-diff</text>
    <text x="84" y="356" font-size="14" class="text">›  karet-syntax</text>
    <text x="84" y="384" font-size="14" class="text">›  karet-theme</text>
    <text x="84" y="412" font-size="14" class="text">›  karet-vcs</text>
    <text x="66" y="454" font-size="14" class="muted">◇  Cargo.toml</text>
    <text x="66" y="482" font-size="14" class="muted">◇  README.md</text>
    <text x="66" y="510" font-size="14" class="muted">◇  AGENTS.md</text>

    <rect x="274" y="88" width="881" height="38" fill="#111827"/>
    <rect x="274" y="124" width="440" height="2" fill="#38bdf8"/>
    <text x="298" y="112" font-size="13" class="bright">editor.rs</text>
    <text x="704" y="112" text-anchor="end" font-size="12" class="muted">WORKTREE</text>
    <text x="738" y="112" font-size="13" class="bright">editor.rs</text>
    <text x="1130" y="112" text-anchor="end" font-size="12" class="muted">INDEX</text>

    <rect x="714" y="126" width="1" height="439" fill="#334155"/>
    <rect x="274" y="126" width="440" height="439" fill="#0b1220"/>
    <rect x="715" y="126" width="440" height="439" fill="#0b1220"/>

    <g font-size="13">
      <text x="290" y="158" class="muted">34</text><text x="327" y="158" class="purple">pub fn</text><text x="383" y="158" class="blue">move_cursor</text><text x="472" y="158" class="text">(&amp;mut self, key: Key) {</text>
      <text x="290" y="185" class="muted">35</text><text x="327" y="185" class="text">  match key {</text>
      <rect x="274" y="198" width="440" height="28" fill="#4c1d2a" opacity=".65"/>
      <text x="290" y="217" class="red">36</text><text x="314" y="217" class="red">−</text><text x="337" y="217" class="text">Key::Home =&gt; self.line_start(),</text>
      <rect x="274" y="226" width="440" height="28" fill="#4c1d2a" opacity=".65"/>
      <text x="290" y="245" class="red">37</text><text x="314" y="245" class="red">−</text><text x="337" y="245" class="text">Key::End =&gt; self.line_end(),</text>
      <text x="290" y="278" class="muted">38</text><text x="327" y="278" class="text">  }</text>
      <text x="290" y="305" class="muted">39</text><text x="327" y="305" class="text">}</text>
      <text x="290" y="353" class="muted">40</text><text x="327" y="353" class="muted">// platform-neutral editor model</text>
      <text x="290" y="380" class="muted">41</text><text x="327" y="380" class="purple">fn</text><text x="352" y="380" class="blue">line_end</text><text x="416" y="380" class="text">(&amp;mut self) {</text>
      <text x="290" y="407" class="muted">42</text><text x="327" y="407" class="text">  self.caret.column = self.width();</text>
      <text x="290" y="434" class="muted">43</text><text x="327" y="434" class="text">}</text>

      <text x="731" y="158" class="muted">34</text><text x="768" y="158" class="purple">pub fn</text><text x="824" y="158" class="blue">move_cursor</text><text x="913" y="158" class="text">(&amp;mut self, key: Key) {</text>
      <text x="731" y="185" class="muted">35</text><text x="768" y="185" class="text">  match key {</text>
      <rect x="715" y="198" width="440" height="28" fill="#123524" opacity=".8"/>
      <text x="731" y="217" class="green">36</text><text x="755" y="217" class="green">+</text><text x="778" y="217" class="text">Key::Home =&gt; self.visual_start(),</text>
      <rect x="715" y="226" width="440" height="28" fill="#123524" opacity=".8"/>
      <text x="731" y="245" class="green">37</text><text x="755" y="245" class="green">+</text><text x="778" y="245" class="text">Key::End =&gt; self.visual_end(),</text>
      <text x="731" y="278" class="muted">38</text><text x="768" y="278" class="text">  }</text>
      <text x="731" y="305" class="muted">39</text><text x="768" y="305" class="text">}</text>
      <text x="731" y="353" class="muted">40</text><text x="768" y="353" class="muted">// platform-neutral editor model</text>
      <text x="731" y="380" class="muted">41</text><text x="768" y="380" class="purple">fn</text><text x="793" y="380" class="blue">visual_end</text><text x="873" y="380" class="text">(&amp;mut self) {</text>
      <text x="731" y="407" class="muted">42</text><text x="768" y="407" class="text">  self.caret.column = self.width();</text>
      <text x="731" y="434" class="muted">43</text><text x="768" y="434" class="text">}</text>
    </g>

    <rect x="274" y="516" width="881" height="49" fill="#111827"/>
    <text x="296" y="546" font-size="13" class="green">✓ 2 files changed</text>
    <text x="1130" y="546" text-anchor="end" font-size="13" class="muted">Rust  Ln 36, Col 18  UTF-8</text>
    <rect x="45" y="565" width="1110" height="30" fill="#0369a1"/>
    <text x="64" y="585" font-size="13" class="bright"> feature/editor-motion</text>
    <text x="1136" y="585" text-anchor="end" font-size="13" class="bright">NORMAL  karet</text>
  </g>
</svg>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hero_is_accessible_standalone_svg() {
        assert!(HERO.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(HERO.contains("<title id=\"title\">"));
        assert!(HERO.contains("<desc id=\"description\">"));
        assert!(HERO.ends_with("</svg>\n"));
        assert!(!HERO.contains("timestamp"));
    }
}

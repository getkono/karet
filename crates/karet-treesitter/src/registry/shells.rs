//! Shell grammar entries kept separate from the stable-id registry table.

use super::GrammarInfo;

pub(super) fn push(grammars: &mut Vec<GrammarInfo>) {
    #[cfg(not(any(
        feature = "lang-zsh",
        feature = "lang-fish",
        feature = "lang-powershell",
        feature = "lang-batch"
    )))]
    let _ = grammars;
    #[cfg(feature = "lang-zsh")]
    grammars.push(GrammarInfo {
        id: super::ZSH,
        name: "Zsh",
        extensions: &["zsh"],
        names: &["zsh"],
        language: || tree_sitter_zsh::LANGUAGE.into(),
        highlights: tree_sitter_zsh::HIGHLIGHT_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-fish")]
    grammars.push(GrammarInfo {
        id: super::FISH,
        name: "Fish",
        extensions: &["fish"],
        names: &["fish"],
        language: tree_sitter_fish::language,
        highlights: tree_sitter_fish::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-powershell")]
    grammars.push(GrammarInfo {
        id: super::POWERSHELL,
        name: "PowerShell",
        extensions: &["ps1", "psm1"],
        names: &["powershell", "pwsh", "ps1"],
        language: || tree_sitter_powershell::LANGUAGE.into(),
        highlights: tree_sitter_powershell::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
    #[cfg(feature = "lang-batch")]
    grammars.push(GrammarInfo {
        id: super::BATCH,
        name: "Batch",
        extensions: &["bat", "cmd"],
        names: &["batch", "bat", "cmd"],
        language: || tree_sitter_batch::LANGUAGE.into(),
        highlights: tree_sitter_batch::HIGHLIGHTS_QUERY,
        injections: None,
        injections_extra: None,
    });
}

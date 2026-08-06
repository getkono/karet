//! Declaration queries for structured data and configuration formats.

use std::borrow::Cow;

use crate::LanguageId;

#[cfg(feature = "lang-ini")]
const INI: &str = r#"
(section (section_name (text) @name)) @definition.namespace
"#;

#[cfg(feature = "lang-properties")]
const PROPERTIES: &str = r#"
(property (key) @name) @definition.property
"#;

#[cfg(feature = "lang-xml")]
const XML: &str = r#"
((element
  (STag
    (Name) @_element
    (Attribute (Name) @_attribute (AttValue) @name))) @definition.object
 (#match? @_attribute "^(id|name)$"))
((element
  (EmptyElemTag
    (Name) @_element
    (Attribute (Name) @_attribute (AttValue) @name))) @definition.object
 (#match? @_attribute "^(id|name)$"))
"#;

pub(super) fn query(_lang: LanguageId) -> Option<Cow<'static, str>> {
    #[cfg(feature = "lang-ini")]
    if _lang == super::super::structured::INI {
        return Some(Cow::Borrowed(INI));
    }
    #[cfg(feature = "lang-properties")]
    if _lang == super::super::structured::PROPERTIES {
        return Some(Cow::Borrowed(PROPERTIES));
    }
    #[cfg(feature = "lang-xml")]
    if _lang == super::super::XML {
        return Some(Cow::Borrowed(XML));
    }
    None
}

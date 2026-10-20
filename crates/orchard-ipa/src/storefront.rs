//! App Store storefront ids and their ISO country codes.

/// Storefront id to lowercase ISO 3166-1 alpha-2 code, for the larger stores.
const STOREFRONTS: &[(u32, &str)] = &[
    (143441, "us"),
    (143442, "fr"),
    (143443, "de"),
    (143444, "gb"),
    (143445, "at"),
    (143446, "be"),
    (143447, "fi"),
    (143448, "gr"),
    (143449, "ie"),
    (143450, "it"),
    (143451, "lu"),
    (143452, "nl"),
    (143453, "pt"),
    (143454, "es"),
    (143455, "ca"),
    (143456, "se"),
    (143457, "no"),
    (143458, "dk"),
    (143459, "ch"),
    (143460, "au"),
    (143461, "nz"),
    (143462, "jp"),
    (143463, "hk"),
    (143464, "sg"),
    (143465, "cn"),
    (143466, "kr"),
    (143467, "in"),
    (143468, "mx"),
    (143469, "ru"),
    (143470, "tw"),
    (143478, "pl"),
    (143480, "tr"),
    (143489, "cz"),
    (143491, "il"),
    (143503, "br"),
    (143505, "ar"),
];

/// Country code for a store front header value such as `143441-1,29`.
pub fn country_for_store_front(store_front: &str) -> Option<&'static str> {
    let id: u32 = store_front.split(['-', ',']).next()?.trim().parse().ok()?;
    STOREFRONTS
        .iter()
        .find(|(sf, _)| *sf == id)
        .map(|(_, cc)| *cc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_header_forms() {
        assert_eq!(country_for_store_front("143441-1,29"), Some("us"));
        assert_eq!(country_for_store_front("143450"), Some("it"));
        assert_eq!(country_for_store_front("999999-1,29"), None);
        assert_eq!(country_for_store_front(""), None);
    }
}

//! YouTube Music preloads Home in its HTML, separately from guide and other browse entries.
use serde_json::Value;

/// Only accept the default Home payload for the requested session/channel. Consent pages,
/// sign-in redirects and pages for a different channel must never replace the listener's Home.
pub(super) fn feed(html: &str, signed_in: bool, page_id: Option<&str>) -> Option<Value> {
    let config = html.split("ytcfg.set(").skip(1).find_map(|part| {
        serde_json::Deserializer::from_str(part)
            .into_iter::<Value>()
            .next()?
            .ok()
            .filter(|cfg| cfg.get("LOGGED_IN").is_some())
    })?;
    if config["LOGGED_IN"].as_bool() != Some(signed_in)
        || config["DELEGATED_SESSION_ID"]
            .as_str()
            .filter(|id| !id.is_empty())
            != page_id
    {
        return None;
    }
    html.split("initialData.push(").skip(1).find_map(|entry| {
        let entry = entry.strip_prefix(r#"{path: '\/browse', params: JSON.parse("#)?;
        let (params, rest) = js_string(entry)?;
        let params: Value = serde_json::from_str(&params).ok()?;
        if params != serde_json::json!({ "browseId": super::HOME }) {
            return None;
        }
        let (data, _) = js_string(rest.strip_prefix("), data: ")?)?;
        serde_json::from_str(&data).ok()
    })
}

/// Decode the single-quoted string literal without executing any page scripts.
fn js_string(input: &str) -> Option<(String, &str)> {
    let mut chars = input.strip_prefix('\'')?.chars();
    let mut out = String::new();
    while let Some(c) = chars.next() {
        match c {
            '\'' => return Some((out, chars.as_str())),
            '\\' => match chars.next()? {
                'x' => out.push(char::from_u32(u32::from(hex(&mut chars, 2)?))?),
                'u' => {
                    let first = hex(&mut chars, 4)?;
                    let ch = if (0xd800..=0xdbff).contains(&first) {
                        chars = chars.as_str().strip_prefix(r"\u")?.chars();
                        char::decode_utf16([first, hex(&mut chars, 4)?])
                            .next()?
                            .ok()?
                    } else {
                        char::from_u32(u32::from(first))?
                    };
                    out.push(ch);
                }
                c @ ('\\' | '\'' | '"' | '/') => out.push(c),
                _ => return None,
            },
            c => out.push(c),
        }
    }
    None
}

fn hex(chars: &mut std::str::Chars<'_>, digits: usize) -> Option<u16> {
    (0..digits).try_fold(0, |n, _| Some(n * 16 + chars.next()?.to_digit(16)? as u16))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: &str = r#"
        ytcfg.set({"LOGGED_IN":true,"DELEGATED_SESSION_ID":"123"});
        initialData.push({path: '\/guide', params: JSON.parse('{}'), data: '{}'});
        initialData.push({path: '\/browse', params: JSON.parse('{"browseId":"FEmusic_explore"}'), data: '{}'});
        initialData.push({path: '\/browse', params: JSON.parse('\x7b\x22browseId\x22:\x22FEmusic_home\x22\x7d'), data: '\x7b\x22title\x22:\x22星のMix \/ DJ\x27s picks \ud83c\udfb5\x22\x7d'});
    "#;

    #[test]
    fn recovers_only_the_matching_home_and_account() {
        assert_eq!(
            feed(PAGE, true, Some("123")).unwrap()["title"],
            "星のMix / DJ's picks 🎵"
        );
        assert!(feed(PAGE, true, None).is_none());
        assert!(feed(PAGE, true, Some("456")).is_none());
        assert!(feed(&PAGE.replace("true", "false"), true, Some("123")).is_none());
        assert!(feed(&PAGE.replace("FEmusic_home", "unknown"), true, Some("123")).is_none());
        assert!(feed(&PAGE.replace(r"\x27", r"\xZZ"), true, Some("123")).is_none());
    }
}

use serde_json::Value;
use url::Url;

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountProfile {
    pub id: String,
    pub name: String,
    pub thumbnail: Option<String>,
    pub followers: Option<u64>,
    pub url: Option<String>,
}

pub(crate) fn parse_account_profile(value: &Value) -> Result<AccountProfile> {
    let username = match value.get("username").filter(|value| !value.is_null()) {
        Some(value) => Some(
            value
                .as_str()
                .filter(|id| valid_identity(id))
                .ok_or(Error::Invalid("account profile identity"))?,
        ),
        None => None,
    };
    let uri_id = match value.get("uri").filter(|value| !value.is_null()) {
        Some(value) => Some(
            value
                .as_str()
                .and_then(user_uri_id)
                .ok_or(Error::Invalid("account profile identity"))?,
        ),
        None => None,
    };
    if username
        .zip(uri_id.as_deref())
        .is_some_and(|(username, uri_id)| username != uri_id)
    {
        return Err(Error::Invalid("account profile identity"));
    }
    let id = username
        .or(uri_id.as_deref())
        .ok_or(Error::Invalid("account profile identity"))?;
    let name = value["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(id)
        .to_owned();
    let thumbnail = value
        .pointer("/avatar/sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|image| {
            let url = public_url(image["url"].as_str()?)?;
            Some((image["width"].as_u64().unwrap_or(0), url.to_string()))
        })
        .max_by_key(|(width, _)| *width)
        .map(|(_, url)| url);
    Ok(AccountProfile {
        id: id.to_owned(),
        name,
        thumbnail,
        followers: None,
        url: profile_url(id),
    })
}

fn valid_identity(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)
}

fn user_uri_id(uri: &str) -> Option<String> {
    let encoded = uri.strip_prefix("spotify:user:")?;
    if encoded.len() > 512 * 3 {
        return None;
    }
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut bytes = encoded.bytes();
    while let Some(byte) = bytes.next() {
        decoded.push(if byte == b'%' {
            let high = char::from(bytes.next()?).to_digit(16)?;
            let low = char::from(bytes.next()?).to_digit(16)?;
            (high * 16 + low) as u8
        } else {
            byte
        });
    }
    String::from_utf8(decoded)
        .ok()
        .filter(|id| valid_identity(id))
}

fn public_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.port_or_known_default() == Some(443)
        && url.fragment().is_none())
    .then_some(url)
}

fn profile_url(id: &str) -> Option<String> {
    let mut url = Url::parse("https://open.spotify.com/user/").ok()?;
    url.path_segments_mut().ok()?.pop_if_empty().push(id);
    Some(url.to_string())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn web_profile_reads_returned_identity_and_largest_public_avatar() {
        let profile = parse_account_profile(&json!({
            "uri": "spotify:user:user123", "username": "user123", "name": "My Name",
            "avatar": {"sources": [
                {"url": "https://i.scdn.co/image/small", "width": 64},
                {"url": "https://i.scdn.co/image/large", "width": 640},
                {"url": "https://user:password@example.test/image", "width": 2000}
            ]},
            "avatarBackgroundColor": 0, "accountId": "account123", "socialHandle": null
        }))
        .unwrap();
        assert_eq!(profile.id, "user123");
        assert_eq!(profile.name, "My Name");
        assert_eq!(
            profile.thumbnail.as_deref(),
            Some("https://i.scdn.co/image/large")
        );
        assert!(profile.followers.is_none());
        assert_eq!(
            profile.url.as_deref(),
            Some("https://open.spotify.com/user/user123")
        );
    }

    #[test]
    fn optional_web_profile_fields_stay_unknown() {
        for value in [
            json!({"username": "user123", "name": null}),
            json!({"uri": "spotify:user:user123", "avatar": null}),
            json!({"username": "user123", "uri": null, "name": " "}),
        ] {
            let profile = parse_account_profile(&value).unwrap();
            assert_eq!(profile.id, "user123");
            assert_eq!(profile.name, "user123");
            assert!(profile.thumbnail.is_none());
            assert!(profile.followers.is_none());
            assert_eq!(
                profile.url.as_deref(),
                Some("https://open.spotify.com/user/user123")
            );
        }
    }

    #[test]
    fn profile_urls_encode_the_returned_identity_once() {
        let profile = parse_account_profile(&json!({
            "username": "a+b/c ?#é",
            "uri": "spotify:user:a%2Bb%2Fc%20%3F%23%C3%A9"
        }))
        .unwrap();
        assert_eq!(profile.id, "a+b/c ?#é");
        assert_eq!(
            profile.url.as_deref(),
            Some("https://open.spotify.com/user/a+b%2Fc%20%3F%23%C3%A9")
        );
        let uri_only = parse_account_profile(&json!({"uri": "spotify:user:a+b%2Fc"})).unwrap();
        assert_eq!(uri_only.id, "a+b/c");
    }

    #[test]
    fn profile_identity_rejects_missing_wrong_type_and_conflicting_values() {
        for value in [
            json!(null),
            json!({"name": "No identity"}),
            json!({"username": 12}),
            json!({"username": ""}),
            json!({"username": " "}),
            json!({"username": "user\n123"}),
            json!({"username": "x".repeat(513)}),
            json!({"username": "user123", "uri": 12}),
            json!({"uri": "spotify:artist:user123", "name": "Artist"}),
            json!({"username": "user123", "uri": "spotify:artist:user123"}),
            json!({"username": "user123", "uri": "spotify:user:other"}),
            json!({"uri": "spotify:user:"}),
            json!({"uri": "spotify:user:bad%"}),
            json!({"uri": "spotify:user:bad%xy"}),
            json!({"uri": "spotify:user:bad%FF"}),
            json!({"uri": "spotify:user:bad%0A"}),
        ] {
            assert!(parse_account_profile(&value).is_err());
        }
    }

    #[test]
    fn avatar_urls_reject_unsafe_schemes_credentials_ports_and_fragments() {
        let profile = parse_account_profile(&json!({
            "username": "user123",
            "avatar": {"sources": [
                {"url": "http://i.scdn.co/image/plain", "width": 2000},
                {"url": "javascript:alert(1)", "width": 2000},
                {"url": "https://user:password@example.test/image", "width": 2000},
                {"url": "https://i.scdn.co:444/image/other-port", "width": 2000},
                {"url": "https://i.scdn.co/image/fragment#part", "width": 2000},
                {"url": "//i.scdn.co/image/relative", "width": 2000}
            ]},
            "external_urls": {"spotify": "https://example.test/malicious"},
            "followers": {"total": 999}
        }))
        .unwrap();
        assert!(profile.thumbnail.is_none());
        assert!(profile.followers.is_none());
        assert_eq!(
            profile.url.as_deref(),
            Some("https://open.spotify.com/user/user123")
        );
    }
}

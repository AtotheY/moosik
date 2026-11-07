use data_encoding::BASE32_NOPAD;
use hmac::{Hmac, Mac};
use reqwest::{Response, header::HeaderValue};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::Sha1;
use url::Url;

use crate::{Error, Result, http_client, now_ms, random_device_id};

pub const WEB_CLIENT_ID: &str = "d8a5ed958d274c2e8ee717e6a4b0971d";
const APP_VERSION: &str = "1.3.5.316.gd873ee9db135";
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/156.0.0.0 Safari/537.36";
const PARAMETERS_URL: &str = "https://api.github.com/gists/22ed9c6ba463899e933427f7de1f0eef";
const JSON_LIMIT: usize = 64 * 1024;
const TOKEN_LIMIT: usize = 16 * 1024;
const EARLY_REFRESH_MS: u64 = 60_000;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebSession {
    pub cookie: String,
    pub access_token: String,
    pub client_token: String,
    pub client_id: String,
    pub device_id: String,
    pub expires_at: u64,
    pub client_token_expires_at: u64,
    pub app_version: String,
    pub user_agent: String,
}

struct Parameters {
    version: u32,
    secret: Vec<u8>,
}

pub async fn web_session(cookie: &str, cached: Option<&WebSession>) -> Result<WebSession> {
    let cookie = cookie.trim_matches([' ', '\t']);
    let cookie_value = cookie_header(cookie)?;
    let now = now_ms()?;
    let account = cached_account(cookie, cached, now);
    let device = cached.filter(|cached| validate_device(cached).is_ok());
    if let Some(account) = account.filter(|_| reusable_device(device, now).is_some()) {
        let mut session = account.clone();
        session.cookie = cookie.to_owned();
        return Ok(session);
    }
    let http = http_client()?;
    let (access_token, client_id, expires_at) = if let Some(account) = account {
        (
            account.access_token.clone(),
            account.client_id.clone(),
            account.expires_at,
        )
    } else {
        let parameters = fetch_json(
            http.get(PARAMETERS_URL).header("user-agent", USER_AGENT),
            "web token parameters",
        )
        .await?;
        let parameters = parse_parameters(&parameters)?;
        let clock = fetch_json(
            http.get("https://open.spotify.com/api/server-time")
                .header("user-agent", USER_AGENT),
            "web server time",
        )
        .await?;
        let server_time = clock["serverTime"]
            .as_u64()
            .filter(|seconds| *seconds > 0 && *seconds <= 4_102_444_800)
            .ok_or(Error::Invalid("web server time"))?;
        let code = totp(&parameters.secret, server_time)?;
        let mut url = Url::parse("https://open.spotify.com/api/token")
            .map_err(|_| Error::Invalid("web token endpoint"))?;
        url.query_pairs_mut()
            .append_pair("reason", "transport")
            .append_pair("productType", "web-player")
            .append_pair("totp", &code)
            .append_pair("totpServer", &code)
            .append_pair("totpVer", &parameters.version.to_string());
        let authorization = fetch_json(
            http.get(url)
                .header("user-agent", USER_AGENT)
                .header("cookie", cookie_value),
            "web authorization",
        )
        .await?;
        parse_authorization(&authorization, now_ms()?)?
    };
    let device_id = device
        .map(|cached| cached.device_id.clone())
        .unwrap_or_else(random_device_id);
    let refreshed_at = now_ms()?;
    let (client_token, client_token_expires_at, app_version, user_agent) = if let Some(cached) =
        reusable_device(device, refreshed_at).filter(|cached| cached.client_id == client_id)
    {
        (
            cached.client_token.clone(),
            cached.client_token_expires_at,
            cached.app_version.clone(),
            cached.user_agent.clone(),
        )
    } else {
        let response = fetch_json(
            http.post("https://clienttoken.spotify.com/v1/clienttoken")
                .header("user-agent", USER_AGENT)
                .header("accept", "application/json")
                .header("origin", "https://open.spotify.com")
                .header("referer", "https://open.spotify.com/")
                .json(&json!({
                    "request_type": "REQUEST_CLIENT_DATA",
                    "client_data": {
                        "client_version": APP_VERSION,
                        "client_id": client_id,
                        "js_sdk_data": {
                            "device_brand": "unknown", "device_model": "unknown",
                            "os": "windows", "os_version": "10.0.0",
                            "device_id": device_id, "device_type": "computer"
                        }
                    }
                })),
            "web client token",
        )
        .await?;
        let (token, expiry) = parse_client_token(&response, now_ms()?)?;
        (token, expiry, APP_VERSION.to_owned(), USER_AGENT.to_owned())
    };
    Ok(WebSession {
        cookie: cookie.to_owned(),
        access_token,
        client_token,
        client_id,
        device_id,
        expires_at,
        client_token_expires_at,
        app_version,
        user_agent,
    })
}

pub(crate) fn cookie_header(cookie: &str) -> Result<HeaderValue> {
    if cookie.is_empty() || cookie.len() > TOKEN_LIMIT || auth_cookie(cookie).is_none() {
        return Err(Error::Invalid("web cookie"));
    }
    let mut value = HeaderValue::from_str(cookie).map_err(|_| Error::Invalid("web cookie"))?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn validate_web_session(session: &WebSession) -> Result<()> {
    cookie_header(&session.cookie)?;
    validate_token(&session.access_token, "web access token")?;
    validate_device(session)
}

fn auth_cookie(cookie: &str) -> Option<&str> {
    let mut auth = None;
    for part in cookie.split(';') {
        let Some((name, value)) = part.trim_matches([' ', '\t']).split_once('=') else {
            continue;
        };
        if name == "sp_dc" {
            if value.is_empty() || auth.is_some() {
                return None;
            }
            auth = Some(value);
        }
    }
    auth
}

fn cached_account<'a>(
    cookie: &str,
    cached: Option<&'a WebSession>,
    now: u64,
) -> Option<&'a WebSession> {
    cached.filter(|cached| {
        cached.client_id == WEB_CLIENT_ID
            && cached.expires_at > now.saturating_add(EARLY_REFRESH_MS)
            && cookie_header(&cached.cookie).is_ok()
            && validate_token(&cached.access_token, "web access token").is_ok()
            && auth_cookie(&cached.cookie) == auth_cookie(cookie)
    })
}

fn reusable_device(cached: Option<&WebSession>, now: u64) -> Option<&WebSession> {
    cached.filter(|cached| {
        cached.client_token_expires_at > now.saturating_add(EARLY_REFRESH_MS)
            && validate_device(cached).is_ok()
    })
}

fn validate_device(session: &WebSession) -> Result<()> {
    validate_token(&session.client_token, "web client token")?;
    if session.client_id != WEB_CLIENT_ID
        || session.device_id.is_empty()
        || session.device_id.len() > 128
        || !session
            .device_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        || session.app_version.is_empty()
        || session.app_version.len() > 128
        || session.user_agent.is_empty()
        || session.user_agent.len() > 1024
        || HeaderValue::from_str(&session.app_version).is_err()
        || HeaderValue::from_str(&session.user_agent).is_err()
    {
        return Err(Error::Invalid("web session"));
    }
    Ok(())
}

fn validate_token(token: &str, stage: &'static str) -> Result<()> {
    if token.is_empty() || token.len() > TOKEN_LIMIT || HeaderValue::from_str(token).is_err() {
        return Err(Error::Invalid(stage));
    }
    Ok(())
}

async fn fetch_json(request: reqwest::RequestBuilder, stage: &'static str) -> Result<Value> {
    let response = request.send().await.map_err(|_| Error::Network(stage))?;
    if !response.status().is_success() {
        return Err(Error::Http {
            stage,
            status: response.status().as_u16(),
        });
    }
    let bytes = bounded_body(response, stage).await?;
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid(stage))
}

async fn bounded_body(mut response: Response, stage: &'static str) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > JSON_LIMIT as u64)
    {
        return Err(Error::Invalid("web response length"));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| Error::Network(stage))? {
        if chunk.len() > JSON_LIMIT.saturating_sub(bytes.len()) {
            return Err(Error::Invalid("web response length"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn parse_parameters(value: &Value) -> Result<Parameters> {
    let file = &value["files"]["nuances.json"];
    if file["truncated"].as_bool() == Some(true) {
        return Err(Error::Invalid("web token parameters"));
    }
    let content = file["content"]
        .as_str()
        .filter(|content| content.len() <= 16 * 1024)
        .ok_or(Error::Invalid("web token parameters"))?;
    let parameters: Value =
        serde_json::from_str(content).map_err(|_| Error::Invalid("web token parameters"))?;
    let entries = parameters
        .as_array()
        .filter(|entries| !entries.is_empty() && entries.len() <= 32)
        .ok_or(Error::Invalid("web token parameters"))?;
    let mut latest = None;
    for entry in entries {
        let version = entry["v"]
            .as_u64()
            .filter(|version| *version > 0 && *version <= u16::MAX as u64)
            .ok_or(Error::Invalid("web token parameters"))? as u32;
        let secret = entry["s"]
            .as_str()
            .filter(|secret| !secret.is_empty() && secret.len() <= 256)
            .ok_or(Error::Invalid("web token parameters"))?;
        let secret = BASE32_NOPAD
            .decode(secret.trim_end_matches('=').to_ascii_uppercase().as_bytes())
            .map_err(|_| Error::Invalid("web token parameters"))?;
        if secret.is_empty() {
            return Err(Error::Invalid("web token parameters"));
        }
        if latest
            .as_ref()
            .is_none_or(|prior: &Parameters| prior.version < version)
        {
            latest = Some(Parameters { version, secret });
        }
    }
    latest.ok_or(Error::Invalid("web token parameters"))
}

fn totp(secret: &[u8], seconds: u64) -> Result<String> {
    let mut hmac =
        Hmac::<Sha1>::new_from_slice(secret).map_err(|_| Error::Invalid("web token parameters"))?;
    hmac.update(&(seconds / 30).to_be_bytes());
    let digest = hmac.finalize().into_bytes();
    let offset = usize::from(digest[digest.len() - 1] & 15);
    let code = u32::from_be_bytes(digest[offset..offset + 4].try_into().unwrap()) & 0x7fff_ffff;
    Ok(format!("{:06}", code % 1_000_000))
}

fn parse_authorization(value: &Value, now: u64) -> Result<(String, String, u64)> {
    if value["isAnonymous"].as_bool() != Some(false) {
        return Err(Error::AuthorizationDenied);
    }
    let token = value["accessToken"]
        .as_str()
        .ok_or(Error::Invalid("web access token"))?;
    validate_token(token, "web access token")?;
    let client_id = value["clientId"]
        .as_str()
        .filter(|client_id| *client_id == WEB_CLIENT_ID)
        .ok_or(Error::Invalid("web client identity"))?;
    let expires_at = value["accessTokenExpirationTimestampMs"]
        .as_u64()
        .filter(|expires_at| *expires_at > now && *expires_at <= now.saturating_add(86_400_000))
        .ok_or(Error::Invalid("web access expiry"))?;
    Ok((token.to_owned(), client_id.to_owned(), expires_at))
}

fn parse_client_token(value: &Value, now: u64) -> Result<(String, u64)> {
    let granted = &value["granted_token"];
    let token = granted["token"].as_str().ok_or(Error::ClientToken)?;
    validate_token(token, "web client token")?;
    let expires_in = granted["expires_after_seconds"]
        .as_u64()
        .filter(|seconds| *seconds > 0 && *seconds <= 31_536_000)
        .ok_or(Error::Invalid("web client token expiry"))?;
    let expiry = now
        .checked_add(expires_in * 1000)
        .ok_or(Error::Invalid("web client token expiry"))?;
    Ok((token.to_owned(), expiry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cached_session(now: u64) -> WebSession {
        WebSession {
            cookie: "sp_dc=test-cookie".into(),
            access_token: "test-access".into(),
            client_token: "test-client".into(),
            client_id: WEB_CLIENT_ID.into(),
            device_id: "test-device".into(),
            expires_at: now + 3_600_000,
            client_token_expires_at: now + 7_200_000,
            app_version: APP_VERSION.into(),
            user_agent: USER_AGENT.into(),
        }
    }

    #[test]
    fn totp_matches_standard_sha1_vectors() {
        let secret = b"12345678901234567890";
        assert_eq!(totp(secret, 59).unwrap(), "287082");
        assert_eq!(totp(secret, 1_111_111_109).unwrap(), "081804");
        assert_eq!(totp(secret, 2_000_000_000).unwrap(), "279037");
    }

    #[test]
    fn public_parameters_are_bounded_and_validated() {
        let valid = json!({"files":{"nuances.json":{"content":"[{\"v\":61,\"s\":\"GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ\"}]","truncated":false}}});
        let parsed = parse_parameters(&valid).unwrap();
        assert_eq!(parsed.version, 61);
        assert_eq!(parsed.secret, b"12345678901234567890");
        assert!(
            parse_parameters(
                &json!({"files":{"nuances.json":{"content":"[{\"v\":61,\"s\":\"bad!\"}]"}}})
            )
            .is_err()
        );
        assert!(parse_parameters(&json!({"files":{"nuances.json":{"content":"[]"}}})).is_err());
        assert!(
            parse_parameters(&json!({"files":{"nuances.json":{"content":"[]","truncated":true}}}))
                .is_err()
        );
    }

    #[test]
    fn secret_values_do_not_reach_parse_errors() {
        let marker = "private-cookie-marker";
        let error = cookie_header(&format!("sp_dc={marker}\r\nother=value"))
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "Spotify returned invalid web cookie");
        assert!(!error.to_string().contains(marker));
        let error = parse_authorization(&json!({"isAnonymous":false,"accessToken":marker,"clientId":"invalid-client","accessTokenExpirationTimestampMs":2000}),1000).err().unwrap();
        assert!(!error.to_string().contains(marker));
        assert_eq!(
            error.to_string(),
            "Spotify returned invalid web client identity"
        );
    }

    #[tokio::test]
    async fn valid_cache_retains_client_and_device_identity() {
        let now = now_ms().unwrap();
        let mut session = cached_session(now);
        session.cookie = "sp_key=old; sp_dc=test-cookie; other=old".into();
        let incoming = "other=new; sp_dc=test-cookie; sp_key=new";
        let reused = web_session(incoming, Some(&session)).await.unwrap();
        assert_eq!(reused.cookie, incoming);
        assert_eq!(reused.access_token, session.access_token);
        assert_eq!(reused.device_id, session.device_id);
        assert_eq!(reused.client_token, session.client_token);
        assert_eq!(
            reused.client_token_expires_at,
            session.client_token_expires_at
        );
    }

    #[test]
    fn account_changes_keep_device_tokens_without_reusing_access() {
        let now = 1000;
        let session = cached_session(now);
        assert!(cached_account("sp_dc=another-account", Some(&session), now).is_none());
        let device = reusable_device(Some(&session), now).unwrap();
        assert_eq!(device.client_token, session.client_token);
        assert_eq!(device.device_id, session.device_id);
        assert_eq!(device.app_version, session.app_version);
        assert_eq!(device.user_agent, session.user_agent);
    }

    #[test]
    fn expired_account_cache_keeps_its_valid_device_token() {
        let now = 1000;
        let mut session = cached_session(now);
        session.expires_at = now + EARLY_REFRESH_MS;
        assert!(cached_account(&session.cookie, Some(&session), now).is_none());
        assert!(reusable_device(Some(&session), now).is_some());
        session.access_token.clear();
        session.cookie.clear();
        assert!(reusable_device(Some(&session), now).is_some());
    }

    #[test]
    fn device_cache_checks_expiry_and_identity_context() {
        let now = 1000;
        let mut session = cached_session(now);
        session.client_token_expires_at = now + EARLY_REFRESH_MS;
        assert!(reusable_device(Some(&session), now).is_none());
        session = cached_session(now);
        session.client_id = "other-client".into();
        assert!(reusable_device(Some(&session), now).is_none());
        session = cached_session(now);
        session.app_version.clear();
        assert!(reusable_device(Some(&session), now).is_none());
        session = cached_session(now);
        session.device_id = "bad/device".into();
        assert!(reusable_device(Some(&session), now).is_none());
    }

    #[test]
    fn duplicate_auth_cookies_do_not_match_an_account() {
        let now = 1000;
        let session = cached_session(now);
        for cookie in [
            "sp_dc=test-cookie; sp_dc=another-account",
            "sp_dc=test-cookie; sp_dc=test-cookie",
            "sp_dc=; sp_key=other",
        ] {
            assert!(cookie_header(cookie).is_err());
            assert!(cached_account(cookie, Some(&session), now).is_none());
        }
    }

    #[test]
    fn client_token_expiry_is_checked() {
        let valid = json!({"granted_token":{"token":"test-client","expires_after_seconds":3600}});
        assert_eq!(parse_client_token(&valid, 1000).unwrap().1, 3_601_000);
        assert!(
            parse_client_token(
                &json!({"granted_token":{"token":"test-client","expires_after_seconds":0}}),
                1000
            )
            .is_err()
        );
        assert!(
            parse_client_token(
                &json!({"granted_token":{"token":"test-client","expires_after_seconds":u64::MAX}}),
                1000
            )
            .is_err()
        );
    }
}

//! Debug helper: probe a URL with the same client the resolver uses, to compare against curl.
use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let url = std::env::args().nth(1).expect("usage: probe-url <url>");
    let no_redirect = std::env::var_os("NO_REDIRECT").is_some();

    let mut b = reqwest::Client::builder();
    if no_redirect {
        b = b.redirect(reqwest::redirect::Policy::none());
    }
    let client = b.build()?;

    let resp = client
        .get(&url)
        .header("Range", "bytes=0-65535")
        .send()
        .await?;

    println!("status: {}", resp.status());
    println!(
        "final url: {}",
        &resp.url().as_str()[..80.min(resp.url().as_str().len())]
    );
    for (k, v) in resp.headers().iter().take(12) {
        println!("  {k}: {}", v.to_str().unwrap_or("<binary>"));
    }
    let body = resp.bytes().await?;
    println!("body bytes: {}", body.len());
    Ok(())
}

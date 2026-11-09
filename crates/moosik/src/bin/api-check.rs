//! Does YouTube Music search work right now, the way the app does it?
//!
//! The counterpart to `resolve-spike`: that one proves a track can be turned into audio, this one
//! proves a query can be turned into results. Run it when search stops working.
//!
//! It goes through [`moosik::api::Api`] rather than calling `ytmapi-rs` directly, so it exercises the
//! ranked path the app actually takes — our own parser over the raw response. A canary that tested
//! the typed queries would be watching the *fallback*, and would stay green through exactly the
//! kind of breakage this is here to catch.
use anyhow::{Context, Result, bail};
use moosik::api::Api;
use moosik::model::Entry;

#[tokio::main]
async fn main() -> Result<()> {
    moosik::init_tracing("warn,moosik=warn");

    let query = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "radiohead nude".into());

    let api = Api::new().context("could not build the API client")?;

    // Sign-in, when a browser can supply it. `MOOSIK_IMPORT_BROWSER=brave` exercises the path that
    // actually works; without it this runs anonymously, as it always has.
    if let Ok(browser) = std::env::var("MOOSIK_IMPORT_BROWSER") {
        match api.sign_in_from_browser(&browser).await {
            Ok(account) => println!("signed in from {browser} as {account}"),
            Err(e) => println!("sign-in from {browser} failed: {e:#}"),
        }
    } else if api.is_signed_in().await {
        // A stored cookie is a claim, not a session — the same check the app makes at startup.
        match api.verify_session().await {
            Ok(Some(account)) => println!("stored session OK: {account}"),
            Ok(None) => {
                println!("stored session is no longer recognised; continuing anonymously");
                api.forget_session().await;
            }
            Err(e) => println!("could not check the stored session ({e:#}); keeping it"),
        }
    }
    // Downloads follow this.
    if api.is_signed_in().await {
        match api.is_premium().await {
            Ok(p) => println!("premium: {p}"),
            Err(e) => println!("premium: could not check ({e:#})"),
        }
    }

    // How long Strict Home order takes against the live feed, for choosing its time limit:
    // `MOOSIK_HOME_MEASURE=<runs>` runs the ordinary load and then strict loads for a few orders,
    // `<runs>` times each, and prints only timings and section names. Nothing else is checked.
    if let Some(runs) = std::env::var("MOOSIK_HOME_MEASURE")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
    {
        measure_home(&api, runs).await;
        return Ok(());
    }

    // One specific playlist by id, for chasing a shape that only sometimes arrives.
    if let Some(id) = std::env::var_os("MOOSIK_PLAYLIST_ID") {
        let id = id.to_string_lossy().into_owned();
        match api.playlist(&id).await {
            Ok(pl) => println!(
                "playlist {id}: '{}' — {} tracks parsed, header says {:?}",
                pl.title,
                pl.tracks.len(),
                pl.track_count
            ),
            Err(e) => println!("playlist {id}: FAILED {e:#}"),
        }
    }

    let started = std::time::Instant::now();
    let results = api
        .search(&query)
        .await
        .with_context(|| format!("search failed for {query:?}"))?;
    let elapsed = started.elapsed();

    println!(
        "search OK in {:.0} ms: {} results for {query:?}",
        elapsed.as_secs_f64() * 1000.0,
        results.items.len(),
    );
    for item in results.items.iter().take(8) {
        let sub = match item {
            Entry::Song(t) | Entry::Video(t) => t.artist_line(),
            Entry::Artist(a) => a.subscribers.clone().unwrap_or_default(),
            Entry::Album(a) => a.artist.clone(),
            Entry::Playlist(p) => p.author.clone(),
            Entry::Mood(_) => String::new(),
        };
        println!("  {:<9} {:<42} {}", item.kind(), item.title(), sub);
    }

    if results.items.is_empty() {
        bail!("search returned nothing — YouTube changed something");
    }

    // Start Mix: radio for the first playable result. Signed out, so this stays green with no
    // session — and it goes through our own `next` parser, not a typed fallback.
    if let Some(seed) = results.playable().first().map(|t| t.video_id.clone()) {
        let mix = api
            .mix(&seed)
            .await
            .with_context(|| format!("could not start a mix for {seed}"))?;
        println!();
        println!(
            "mix OK: {} tracks for {seed}{}",
            mix.tracks.len(),
            mix.next
                .as_ref()
                .map(|_| ", river continues")
                .unwrap_or(", river ends here"),
        );
        if mix.tracks.len() < 20 {
            bail!("mix came back short — YouTube changed something");
        }
        if mix.tracks.iter().any(|t| t.video_id.is_empty()) {
            bail!("a mix track has no video id");
        }
        // Scrobble: the `player` response must carry both beacon URLs. Fired when signed
        // in, this is what writes the history item — so the canary fires it, and proves
        // the whole scrobble path end to end. (That writes one history item per run.)
        match api.playback_tracking(&seed).await {
            Ok(Some(tracking)) => {
                let short = |url: &str| {
                    url.chars().take(80).collect::<String>()
                        + if url.len() > 80 { "…" } else { "" }
                };
                println!();
                println!("tracking OK for {seed}:");
                println!("  playback:  {}", short(&tracking.playback_url));
                println!("  watchtime: {}", short(&tracking.watchtime_base));
                if !tracking.watchtime_base.contains("watchtime") {
                    bail!("watchtime base is not a watchtime url");
                }
                if api.is_signed_in().await {
                    api.report_playback(&tracking.playback_url)
                        .await
                        .context("the playback beacon was refused")?;
                    println!("  playback beacon accepted");
                } else {
                    println!("  anonymous: beacon not fired");
                }
            }
            Ok(None) => println!("\nno tracking urls (anonymous player response)"),
            Err(e) => bail!("player lookup failed: {e:#}"),
        }
        // Autoplay: the river must extend past its first page.
        if let Some(token) = mix.next {
            let more = api
                .mix_more(&token)
                .await
                .context("could not follow the mix continuation")?;
            println!("mix continuation OK: {} more tracks", more.tracks.len());
            if more.tracks.is_empty() {
                bail!("mix continuation came back empty — YouTube changed something");
            }
        }
    }

    // The other parser that had to be taken over: `ytmapi-rs` discards the per-song covers on an
    // artist page, and reads a subscriber count without its unit. Check the first artist the
    // search turned up, so this canary covers both endpoints we parse ourselves.
    if let Some(id) = results.items.iter().find_map(|i| match i {
        Entry::Artist(a) => Some(a.id.clone()),
        _ => None,
    }) {
        let artist = api
            .get_artist(&id)
            .await
            .with_context(|| format!("could not load artist {id}"))?;
        println!();
        println!(
            "artist OK: {} ({id}) — {}",
            artist.name,
            [artist.subscribers.as_deref(), artist.views.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" • ")
        );
        println!(
            "  {} top songs, {} shelves: {}",
            artist.top_songs.len(),
            artist.shelves.len(),
            artist
                .shelves
                .iter()
                .map(|s| format!("{} ({})", s.title, s.items.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        for t in artist.top_songs.iter().take(3) {
            let cover = t.thumbnail.as_deref().unwrap_or("(none)");
            println!(
                "  {:<34} {}",
                t.title,
                &cover[cover.len().saturating_sub(28)..]
            );
        }
        // Every song sharing one cover means we are back to painting the artist photo on
        // every row — the exact regression this parser exists to prevent.
        let covers: std::collections::HashSet<_> = artist
            .top_songs
            .iter()
            .filter_map(|t| t.thumbnail.as_deref())
            .collect();
        if artist.top_songs.len() > 1 && covers.len() < 2 {
            bail!("every top song shares one cover — per-song art regressed");
        }
        if artist
            .views
            .as_deref()
            .is_some_and(|v| v.matches("views").count() > 1)
        {
            bail!("the view count says 'views' twice");
        }
        // Flattening the carousels is what merged albums with singles and lost five other shelves.
        if artist.shelves.len() < 2 {
            bail!(
                "only {} artist shelves — the carousels are being flattened",
                artist.shelves.len()
            );
        }
    }

    // The library — four browse ids, two different layouts, only meaningful when signed in.
    if api.is_signed_in().await {
        let lib = api
            .library(Default::default())
            .await
            .context("could not load the library")?;
        println!();
        println!(
            "library OK (page 1): {} songs, {} artists, {} albums, {} playlists",
            lib.songs.len(),
            lib.artists.len(),
            lib.albums.len(),
            lib.playlists.len()
        );
        // The save picker is exactly the editable ones. If the `EDIT` menu item moves, the
        // picker silently empties and "Save to playlist" disappears with no error anywhere.
        let editable = lib.playlists.items.iter().filter(|p| p.editable).count();
        println!(
            "  playlists: {editable} of {} can be added to",
            lib.playlists.len()
        );
        if !lib.playlists.is_empty() && editable == 0 {
            bail!("no playlist is marked editable — the save picker would be empty");
        }
        // Two of the four sections are `gridRenderer`, which `ytmapi-rs` cannot read; if albums
        // and playlists are empty while songs are not, that layout has stopped parsing.
        // Library rows put the album in a different column from search rows and the duration in
        // a fixed column. Reading a fixed index silently produced rows with neither.
        let with_album = lib
            .songs
            .items
            .iter()
            .filter(|t| t.album_id.is_some())
            .count();
        let with_cover = lib
            .songs
            .items
            .iter()
            .filter(|t| t.thumbnail.is_some())
            .count();
        println!(
            "  {with_album}/{} songs link an album, {with_cover} have a cover",
            lib.songs.len()
        );
        if !lib.songs.is_empty() && with_album == 0 {
            bail!("no library song links an album — the row layout regressed");
        }
        if with_cover < lib.songs.len() {
            bail!(
                "{} library songs have no cover",
                lib.songs.len() - with_cover
            );
        }

        // Two of the four sections are `gridRenderer`, which `ytmapi-rs` cannot read; if albums
        // and playlists are empty while songs are not, that layout has stopped parsing.
        if !lib.songs.is_empty() && lib.albums.is_empty() && lib.playlists.is_empty() {
            bail!("songs loaded but albums and playlists did not — the grid layout regressed");
        }

        // Paging. The library is far larger than one page, and presenting page one as the whole of
        // it is what made every sort look invented.
        // Walk several pages through our own parser, which is what the app does as you scroll.
        if std::env::var_os("MOOSIK_WALK").is_some() {
            let mut token = lib.songs.next.clone();
            let mut total = lib.songs.len();
            let mut sizes = vec![lib.songs.len()];
            let mut page = 1;
            while let Some(t) = token.take() {
                if page >= 8 {
                    println!("  walk: stopped at the {page}-page cap with a token still in hand");
                    break;
                }
                let p = api
                    .library_more(moosik::api::LibrarySection::Songs, t)
                    .await
                    .context("a continuation failed")?;
                let n = p.items.iter().filter(|e| e.track().is_some()).count();
                sizes.push(n);
                total += n;
                page += 1;
                if n == 0 {
                    println!("  walk: page {page} parsed to zero rows — stopping");
                    break;
                }
                token = p.next;
            }
            println!(
                "  walk: {total} songs over {page} pages, per page {sizes:?}, ended={}",
                if token.is_none() {
                    "no more tokens"
                } else {
                    "cap"
                }
            );
        }

        match lib.songs.next.clone() {
            None => println!("  songs: single page, nothing more to fetch"),
            Some(token) => {
                let page = api
                    .library_more(moosik::api::LibrarySection::Songs, token)
                    .await
                    .context("could not load the second page of songs")?;
                let more = page.items.iter().filter(|e| e.track().is_some()).count();
                println!(
                    "  songs: page 1 had {}, page 2 added {more}, more pages: {}",
                    lib.songs.len(),
                    page.next.is_some()
                );
                if more == 0 {
                    bail!("YouTube offered another page of songs and it parsed as empty");
                }
            }
        }
    }

    // Listening history — dated shelves, parsed by the same shelf reader as home. Only
    // meaningful signed in; signed out YouTube answers a sign-in prompt, not shelves.
    if api.is_signed_in().await {
        let history = api
            .history()
            .await
            .context("could not load the history feed")?;
        println!();
        println!(
            "history OK: {} shelves — {}",
            history.len(),
            history
                .iter()
                .map(|s| format!("{} ({})", s.title, s.items.len()))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if history.iter().all(|s| s.items.is_empty()) {
            bail!("history parsed but every shelf is empty — the layout regressed");
        }
    }

    // Playlists, which the sidebar links to — including "Liked music".
    if std::env::var_os("MOOSIK_PLAYLIST").is_some() && api.is_signed_in().await {
        let lib = api.library(Default::default()).await?;
        let mut ids: Vec<String> = vec!["LM".to_owned()];
        ids.extend(lib.playlists.items.iter().take(2).map(|p| p.id.clone()));
        println!();
        for id in ids {
            match api.playlist(&id).await {
                Ok(pl) => println!(
                    "playlist {id}: {} tracks parsed, header says {:?}",
                    pl.tracks.len(),
                    pl.track_count
                ),
                Err(e) => println!("playlist {id}: FAILED {e:#}"),
            }
        }
    }

    // The home feed, the third response we parse ourselves.
    let feed = api
        .home(None, &moosik::model::HomeFetch::Quick)
        .await
        .context("could not load the home feed")?;
    println!();
    println!(
        "home OK: {} shelves, {} chips — {}",
        feed.shelves.len(),
        feed.chips.len(),
        feed.shelves
            .iter()
            .map(|s| format!("{} ({})", s.title, s.items.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if feed.chips.is_empty() {
        println!("  NOTE: no mood-bar chips — the header stopped parsing");
    }
    // A ranked list that is all one type means we fell back to the filtered queries, or that the
    // flat layout stopped parsing. Either is worth knowing about before users find it.
    let kinds: std::collections::HashSet<_> = results.items.iter().map(Entry::kind).collect();
    if kinds.len() == 1 {
        println!(
            "  NOTE: every result is {:?} — the ranked path may have fallen back",
            kinds
        );
    }

    // Explore's three sub-pages, through the parsers the app actually reads.
    let releases = api
        .new_releases()
        .await
        .context("could not load new releases")?;
    println!();
    println!(
        "releases OK: {} shelves — {}",
        releases.len(),
        releases
            .iter()
            .map(|s| format!("{} ({})", s.title, s.items.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if releases.iter().all(|s| s.items.is_empty()) {
        bail!("new releases parsed but every shelf is empty");
    }
    let charts = api.charts(None).await.context("could not load charts")?;
    println!(
        "charts OK: {} ranked sections, {} card shelves",
        charts.ranked.len(),
        charts.shelves.len()
    );
    if charts.ranked.is_empty() && charts.shelves.is_empty() {
        bail!("charts parsed to nothing — the layout regressed");
    }
    for section in charts.ranked.iter().take(2) {
        let with_rank = section.rows.iter().filter(|r| !r.rank.is_empty()).count();
        println!(
            "  {}: {with_rank}/{} rows carry a rank",
            section.title,
            section.rows.len()
        );
    }
    // Ranks live one level down: the landing is carousels of chart playlists, and the rows
    // of a chart playlist carry the rank column. Open the first one and prove it.
    if let Some(id) = charts
        .shelves
        .iter()
        .flat_map(|s| s.items.iter())
        .find_map(|i| match i {
            moosik::model::Entry::Playlist(p) => Some(p.id.clone()),
            _ => None,
        })
    {
        let chart = api
            .playlist(&id)
            .await
            .context("could not open a chart playlist")?;
        let ranked = chart.tracks.iter().filter(|t| t.rank.is_some()).count();
        println!(
            "  chart playlist {}: {ranked}/{} rows carry a rank",
            id,
            chart.tracks.len()
        );
        if !chart.tracks.is_empty() && ranked == 0 {
            bail!("a chart playlist parsed with no ranks — the rank column regressed");
        }
    }
    let moods = api.moods().await.context("could not load moods")?;
    println!(
        "moods OK: {} sections — {}",
        moods.len(),
        moods
            .iter()
            .map(|s| format!("{} ({})", s.name, s.moods.len()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if moods.iter().all(|s| s.moods.is_empty()) {
        bail!("moods parsed but every section is empty");
    }
    Ok(())
}

/// Time the ordinary Home load against strict ones for a few orders. Prints section names and
/// timings only.
async fn measure_home(api: &Api, runs: usize) {
    use moosik::model::{HomeFetch, HomeSection as S, normalise_order};
    use std::time::Instant;

    let keys = |first: &[S]| {
        let mut out: Vec<String> = first.iter().map(|s| s.key().to_owned()).collect();
        out.extend(
            S::DEFAULT
                .iter()
                .filter(|s| !first.contains(s))
                .map(|s| s.key().to_owned()),
        );
        out
    };
    let orders: [(&str, Vec<String>); 3] = [
        ("default order", keys(&[])),
        (
            "mid-feed sections first",
            keys(&[
                S::FeaturedPlaylists,
                S::MusicVideos,
                S::ForgottenFavorites,
                S::NewReleases,
                S::Trending,
                S::AlbumsForYou,
                S::FromYourLibrary,
            ]),
        ),
        (
            "rare sections first",
            keys(&[
                S::LongListens,
                S::LivePerformances,
                S::FromTheCommunity,
                S::CoversAndRemixes,
                S::SimilarTo,
                S::Themed,
                S::ForgottenFavorites,
            ]),
        ),
    ];

    for run in 1..=runs {
        let started = Instant::now();
        match api.home(None, &HomeFetch::Quick).await {
            Ok(feed) => println!(
                "run {run}: ordinary load {:>5} ms, {} shelves",
                started.elapsed().as_millis(),
                feed.shelves.len()
            ),
            Err(e) => println!("run {run}: ordinary load failed: {e:#}"),
        }
        for (name, saved) in &orders {
            let fetch = HomeFetch::strict(&normalise_order(saved), &[]);
            let HomeFetch::Strict { wanted, .. } = &fetch else {
                continue;
            };
            let started = Instant::now();
            match api.home(None, &fetch).await {
                Ok(feed) => {
                    let missing: Vec<_> = wanted
                        .iter()
                        .filter(|w| !w.present(&feed.shelves))
                        .collect();
                    println!(
                        "run {run}: strict, {name:<24} {:>5} ms, {:>2} shelves, more after: {}, missing {missing:?}",
                        started.elapsed().as_millis(),
                        feed.shelves.len(),
                        feed.next.is_some(),
                    );
                }
                Err(e) => println!("run {run}: strict, {name}: failed: {e:#}"),
            }
        }
    }
}

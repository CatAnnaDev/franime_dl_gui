#[path = "../src/voiranime.rs"]
mod voiranime;

#[tokio::test]
#[ignore]
async fn probe_browse() {
    let client = voiranime::VaClient::new().unwrap();
    let list = client.browse_page(2).await.unwrap();
    println!("Page 2: {} animes", list.len());
    for s in &list[..5.min(list.len())] {
        println!("  - {} | {} | img={:?}", s.slug, s.title, s.image);
    }
    assert!(list.len() >= 10);
}

#[tokio::test]
#[ignore]
async fn probe_detail_and_iframe() {
    let client = voiranime::VaClient::new().unwrap();
    let detail = client.anime_detail("07-ghost").await.unwrap();
    println!("07-ghost: {} episodes", detail.episodes.len());
    println!("  score={:?} year={:?} status={:?}", detail.score, detail.year, detail.status);
    println!("  genres={:?}", detail.genres);
    println!("  desc[..200]={}", &detail.description[..detail.description.len().min(200)]);
    assert!(detail.episodes.len() >= 20);
    for e in &detail.episodes[..3.min(detail.episodes.len())] {
        println!("  ep #{} ({}) {}", e.number, e.lang, e.url);
    }
    let ep1 = detail.episodes.first().unwrap();
    let iframe = client.episode_iframe(&ep1.url).await.unwrap();
    println!("iframe ep1: {}", iframe);
    assert!(iframe.starts_with("http"));
}

#[tokio::test]
#[ignore]
async fn probe_raw_bytes() {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36")
        .build()
        .unwrap();
    let resp = client
        .get("https://voir-anime.to/anime/07-ghost/")
        .send()
        .await
        .unwrap();
    let ct = resp.headers().get("content-type").map(|v| v.to_str().unwrap_or("").to_string()).unwrap_or_default();
    let ce = resp.headers().get("content-encoding").map(|v| v.to_str().unwrap_or("").to_string()).unwrap_or_default();
    println!("Content-Type: {}", ct);
    println!("Content-Encoding: {}", ce);
    let bytes = resp.bytes().await.unwrap();
    if let Some(pos) = bytes.windows(6).position(|w| w == b"TERMIN") {
        println!("TERMIN at pos {}", pos);
        let slice = &bytes[pos..pos + 12];
        print!("Hex bytes: ");
        for b in slice {
            print!("{:02x} ", b);
        }
        println!();
        println!("UTF-8 lossy decode: {:?}", String::from_utf8_lossy(slice));
    } else {
        println!("Could not find TERMIN");
    }
}

#[tokio::test]
#[ignore]
async fn probe_episode_sources() {
    let client = voiranime::VaClient::new().unwrap();
    let sources = client
        .episode_sources("https://voir-anime.to/anime/07-ghost/07-ghost-01-vostfr/")
        .await
        .unwrap();
    println!("Found {} sources", sources.len());
    for s in &sources {
        println!("  - {} → {}", s.host, s.iframe);
    }
    assert!(sources.len() >= 3);
}

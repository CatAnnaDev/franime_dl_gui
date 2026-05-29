#[path = "../src/animesama.rs"]
mod animesama;

#[tokio::test]
#[ignore]
async fn probe_sitemap() {
    let client = animesama::AsClient::new().unwrap();
    let list = client.fetch_sitemap().await.unwrap();
    println!("sitemap: {} animes", list.len());
    for (slug, lastmod) in &list[..6.min(list.len())] {
        println!("  - {} | {}", slug, lastmod);
    }
    assert!(list.len() >= 1000);
}

#[tokio::test]
#[ignore]
async fn probe_detail() {
    let client = animesama::AsClient::new().unwrap();
    let detail = client.anime_detail("86-eighty-six").await.unwrap();
    println!("title={} | genres={:?}", detail.title, detail.genres);
    println!("desc[..120]={}", &detail.description[..detail.description.len().min(120)]);
    println!("seasons={}", detail.seasons.len());
    for s in &detail.seasons {
        println!("  [{}] {} épisode(s)", s.title, s.episodes.len());
        if let Some(e) = s.episodes.first() {
            println!(
                "     ep1 vo={} vf={}",
                e.vo.len(),
                e.vf.len()
            );
            for src in e.vo.iter().chain(e.vf.iter()).take(4) {
                println!("       {} -> {}", src.host, src.iframe);
            }
        }
    }
    assert!(!detail.seasons.is_empty());
}

use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::Path;
use tokio::io::AsyncWriteExt;

pub async fn download_file(url: &str, dest: &Path) -> Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch {url}"))?;

    if !resp.status().is_success() {
        bail!("Download failed: HTTP {} for {url}", resp.status());
    }

    let total = resp.content_length().unwrap_or(0);

    let pb = ProgressBar::new(total);
    pb.set_style(
        ProgressStyle::with_template(
            "  {bar:40.cyan/dim} {bytes}/{total_bytes} {bytes_per_sec} {eta}",
        )
        .unwrap()
        .progress_chars("━╸─"),
    );

    let mut file = tokio::fs::File::create(dest).await?;
    let mut stream = resp.bytes_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.with_context(|| "Error reading download stream")?;
        file.write_all(&chunk).await?;
        pb.inc(chunk.len() as u64);
    }

    file.flush().await?;
    pb.finish_and_clear();
    Ok(())
}

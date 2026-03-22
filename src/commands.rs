use std::{collections::HashSet, fs, path::Path};

use color_eyre::eyre::{Context, Result};

use crate::{pdf_strip, pull_changes::PullRequests, remarkable::RemarkableClient};

/// Gets new pull request reviews from GitHub and uploads them to the reMarkable
pub(crate) fn get_reviews_and_send_to_remarkable() -> Result<()> {
    let remarkable =
        RemarkableClient::connect().wrap_err("while syncing, could not connect to reMarkable")?;

    let prs = PullRequests::fetch().wrap_err("while syncing, could not fetch pull requests")?;

    let existing: HashSet<String> = remarkable
        .list_inkrement_documents()
        .wrap_err("while syncing, could not get Inkrement documents")?
        .into_iter()
        .map(|d| d.visible_name)
        .collect();

    for pr in prs.pull_requests {
        let filename = pr.pdf_filename();

        if existing.contains(&filename) {
            println!("Skipping {} (already on tablet)", filename);
            continue;
        }

        println!("Rendering {}...", filename);
        let pdf = pr.render_to_pdf(&prs.reviewer).wrap_err_with(|| {
            format!(
                "while syncing, failed to render {}#{} \"{}\"",
                pr.repo_name, pr.number, pr.title
            )
        })?;

        println!("Uploading {}...", filename);
        remarkable.upload(&filename, &pdf).wrap_err_with(|| {
            format!(
                "while syncing, failed to upload {}#{} \"{}\"",
                pr.repo_name, pr.number, pr.title
            )
        })?;

        println!("Uploaded: {}", filename);
    }

    Ok(())
}

/// Generate PDF output locally without requiring a reMarkable connected
pub(crate) fn generate_local_pdfs(output_dir: &Path) -> Result<()> {
    let prs = PullRequests::fetch()?;

    fs::create_dir_all(output_dir).wrap_err("failed to create output directory")?;

    for pr in prs.pull_requests {
        let pdf = pr.render_to_pdf(&prs.reviewer).wrap_err_with(|| {
            format!(
                "failed to PDF-render {}#{} \"{}\"",
                pr.repo_name, pr.number, pr.title
            )
        })?;
        let filename = pr.pdf_filename();
        pdf.write(output_dir, &filename).wrap_err_with(|| {
            format!(
                "failed to write PDF to file for {}#{} \"{}\"",
                pr.repo_name, pr.number, pr.title
            )
        })?;

        println!("Generated: {}", output_dir.join(&filename).display());
    }

    Ok(())
}

/// Download all inkrement documents from reMarkable, strip source pages, and save locally.
pub(crate) fn download_annotated_pdfs(output_dir: &Path) -> Result<()> {
    let remarkable = RemarkableClient::connect().wrap_err("could not connect to reMarkable")?;

    let docs = remarkable
        .list_inkrement_documents()
        .wrap_err("could not list inkrement documents on reMarkable")?;

    if docs.is_empty() {
        println!("No inkrement documents found on reMarkable.");
        return Ok(());
    }

    fs::create_dir_all(output_dir).wrap_err("failed to create output directory")?;

    for doc in docs {
        println!("Downloading {}...", doc.visible_name);
        let pdf_bytes = remarkable
            .download(&doc.id)
            .wrap_err_with(|| format!("failed to download {}", doc.visible_name))?;

        println!("Stripping source pages...");
        let stripped = pdf_strip::strip_source_pages(&pdf_bytes)
            .wrap_err_with(|| format!("failed to strip source pages from {}", doc.visible_name))?;

        let filename = format!("{}.pdf", doc.visible_name);
        let path = output_dir.join(&filename);
        fs::write(&path, &stripped)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;

        println!("Saved: {}", path.display());
    }

    Ok(())
}

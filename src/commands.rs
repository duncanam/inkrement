use std::{collections::HashSet, fs, path::Path};

use color_eyre::eyre::{Context, Result};

use color_eyre::eyre::eyre;

use crate::{
    annotate::{self, InterpretedReview},
    pdf_strip,
    post_review::{self, ReviewTarget},
    pull_changes::{INKREMENT_TAG, PullRequests},
    remarkable::RemarkableClient,
};

/// Parse the OCR page count from an inkrement filename.
/// Expected format: "#72 repo [sha] p42 inkrement.pdf" -> 42
fn parse_ocr_pages_from_filename(name: &str) -> Result<usize> {
    let after_bracket = name
        .split("] ")
        .nth(1)
        .ok_or_else(|| eyre!("no ] delimiter"))?;
    let pages_str = after_bracket
        .split_once(' ')
        .map(|(p, _)| p)
        .ok_or_else(|| eyre!("no space after page count"))?;
    pages_str
        .strip_prefix('p')
        .ok_or_else(|| eyre!("page count does not start with 'p'"))?
        .parse()
        .wrap_err("invalid page count number")
}

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
        let prefix = pr.filename_prefix();

        if existing.iter().any(|f| f.starts_with(&prefix)) {
            println!("Skipping {} (already on tablet)", prefix);
            continue;
        }

        println!("Rendering {}...", prefix);
        let pdf = pr.render_to_pdf(&prs.reviewer).wrap_err_with(|| {
            format!(
                "while syncing, failed to render {}#{} \"{}\"",
                pr.repo_name, pr.number, pr.title
            )
        })?;

        let filename = pr.pdf_filename(pdf.ocr_page_count);
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
        let filename = pr.pdf_filename(pdf.ocr_page_count);
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

        let ocr_pages = parse_ocr_pages_from_filename(&doc.visible_name).wrap_err_with(|| {
            format!(
                "could not determine OCR page count from filename: {}",
                doc.visible_name
            )
        })?;

        println!("Stripping source pages (keeping {ocr_pages} pages)...");
        let stripped = pdf_strip::strip_source_pages(&pdf_bytes, ocr_pages)
            .wrap_err_with(|| format!("failed to strip source pages from {}", doc.visible_name))?;

        let filename = &doc.visible_name;
        let path = output_dir.join(&filename);
        fs::write(&path, &stripped)
            .wrap_err_with(|| format!("failed to write {}", path.display()))?;

        println!("Saved: {}", path.display());
    }

    Ok(())
}

/// Interpret annotated PDFs in a directory via Claude OCR, saving JSON alongside each PDF.
pub(crate) fn interpret_pdfs(input_dir: &Path) -> Result<()> {
    let entries: Vec<_> = fs::read_dir(input_dir)
        .wrap_err_with(|| format!("failed to read directory {}", input_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.contains(INKREMENT_TAG) && name.ends_with(".pdf")
        })
        .collect();

    if entries.is_empty() {
        println!("No inkrement PDFs found in {}", input_dir.display());
        return Ok(());
    }

    for entry in &entries {
        let pdf_path = entry.path();
        let name = pdf_path.file_name().unwrap().to_string_lossy();

        println!("Interpreting {name}...");
        let review = annotate::interpret_pdf(&pdf_path)
            .wrap_err_with(|| format!("failed to interpret {name}"))?;

        if review.is_empty() {
            println!("Skipping {name} (no annotations found)");
            continue;
        }

        let json = serde_json::to_string_pretty(&review)
            .wrap_err("failed to serialize interpretation to JSON")?;

        let json_path = pdf_path.with_extension("json");
        fs::write(&json_path, &json)
            .wrap_err_with(|| format!("failed to write {}", json_path.display()))?;

        println!(
            "Saved: {} ({} annotations, decision: {:?})",
            json_path.display(),
            review.annotations.len(),
            review.decision,
        );
    }

    Ok(())
}

/// Post reviews from JSON files to GitHub.
pub(crate) fn post_reviews(input_dir: &Path) -> Result<()> {
    let entries: Vec<_> = fs::read_dir(input_dir)
        .wrap_err_with(|| format!("failed to read directory {}", input_dir.display()))?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.contains(INKREMENT_TAG) && name.ends_with(".json")
        })
        .collect();

    if entries.is_empty() {
        println!("No inkrement JSON files found in {}", input_dir.display());
        return Ok(());
    }

    for entry in &entries {
        let json_path = entry.path();
        let name = json_path.file_name().unwrap().to_string_lossy();

        let json =
            fs::read_to_string(&json_path).wrap_err_with(|| format!("failed to read {name}"))?;
        let review: InterpretedReview =
            serde_json::from_str(&json).wrap_err_with(|| format!("failed to parse {name}"))?;

        if review.is_empty() {
            println!("Skipping {name} (no annotations)");
            continue;
        }

        let target = ReviewTarget::from_filename(&name)
            .wrap_err_with(|| format!("failed to parse PR info from {name}"))?;

        println!(
            "Posting review for {}#{} ({} annotations, decision: {:?})...",
            target.repo,
            target.number,
            review.annotations.len(),
            review.decision,
        );

        post_review::post_review(&target, &review).wrap_err_with(|| {
            format!(
                "failed to post review for {}#{}",
                target.repo, target.number
            )
        })?;

        println!("Posted review for {}#{}", target.repo, target.number);
    }

    Ok(())
}

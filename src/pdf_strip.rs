use color_eyre::eyre::{Context, Result};
use lopdf::Document;

/// Strip source pages from an annotated PDF.
///
/// Takes the OCR page count (parsed from the filename) and removes all pages
/// after that point. Returns a new PDF containing only the review content.
pub(crate) fn strip_source_pages(pdf_bytes: &[u8], ocr_page_count: usize) -> Result<Vec<u8>> {
    let mut doc = Document::load_mem(pdf_bytes).wrap_err("failed to parse PDF")?;

    let total_pages = doc.get_pages().len() as u32;
    let keep = ocr_page_count as u32;

    if keep >= total_pages {
        return Ok(pdf_bytes.to_vec());
    }

    let pages_to_delete: Vec<u32> = ((keep + 1)..=total_pages).collect();
    doc.delete_pages(&pages_to_delete);

    let mut output = Vec::new();
    doc.save_to(&mut output)
        .wrap_err("failed to write stripped PDF")?;

    Ok(output)
}

use std::path::Path;

use color_eyre::eyre::{Context, Result, eyre};
use typst::{
    Library, LibraryExt, compile,
    diag::{FileError, FileResult},
    foundations::{Bytes, Datetime, NativeElement, Selector},
    layout::PagedDocument,
    model::HeadingElem,
    syntax::{FileId, Source, VirtualPath},
    text::{Font, FontBook},
    utils::LazyHash,
};
use typst_pdf::PdfOptions;

use crate::review_data::ReviewData;

const TEMPLATE: &str = include_str!("../templates/review.typ");
const DATA_PATH: &str = "review-data.json";
const TEMPLATE_PATH: &str = "review.typ";

// Embed fonts at compile time
const FONT_JETBRAINS_MONO: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");
const FONT_JETBRAINS_MONO_BOLD: &[u8] = include_bytes!("../fonts/JetBrainsMono-Bold.ttf");
const FONT_INTER: &[u8] = include_bytes!("../fonts/Inter-VariableFont_opsz,wght.ttf");

/// Minimal Typst World implementation that serves an embedded template,
/// a JSON data file from memory, and bundled fonts.
struct InkrementWorld {
    library: LazyHash<Library>,
    book: LazyHash<FontBook>,
    fonts: Vec<Font>,
    main_id: FileId,
    data_id: FileId,
    main_source: Source,
    data_bytes: Bytes,
}

impl InkrementWorld {
    /// Create a new Typst world for Inkrement
    fn new(review_data: &ReviewData) -> Result<Self> {
        let json = serde_json::to_string(review_data).map_err(|e| {
            eyre!("failed to serialize review data while creating typst World: {e}")
        })?;

        let main_id = FileId::new(None, VirtualPath::new(TEMPLATE_PATH));
        let data_id = FileId::new(None, VirtualPath::new(DATA_PATH));

        let main_source = Source::new(main_id, TEMPLATE.to_string());
        let data_bytes = Bytes::from_string(json);

        // Load embedded fonts
        let (font_book, fonts) = [FONT_JETBRAINS_MONO, FONT_JETBRAINS_MONO_BOLD, FONT_INTER]
            .into_iter()
            .flat_map(|data| Font::iter(Bytes::new(data)))
            .fold(
                (FontBook::new(), Vec::new()),
                |(mut book, mut fonts), font| {
                    book.push(font.info().clone());
                    fonts.push(font);
                    (book, fonts)
                },
            );

        let library = LazyHash::new(Library::default());
        let book = LazyHash::new(font_book);

        Ok(Self {
            library,
            book,
            fonts,
            main_id,
            data_id,
            main_source,
            data_bytes,
        })
    }
}

impl typst::World for InkrementWorld {
    fn library(&self) -> &LazyHash<Library> {
        &self.library
    }

    fn book(&self) -> &LazyHash<FontBook> {
        &self.book
    }

    fn main(&self) -> FileId {
        self.main_id
    }

    fn source(&self, id: FileId) -> FileResult<Source> {
        if id == self.main_id {
            Ok(self.main_source.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
        }
    }

    fn file(&self, id: FileId) -> FileResult<Bytes> {
        if id == self.data_id {
            Ok(self.data_bytes.clone())
        } else {
            Err(FileError::NotFound(id.vpath().as_rooted_path().into()))
        }
    }

    fn font(&self, index: usize) -> Option<Font> {
        self.fonts.get(index).cloned()
    }

    fn today(&self, _offset: Option<i64>) -> Option<Datetime> {
        None
    }
}

/// A rendered PDF with metadata about its structure.
pub(crate) struct Pdf {
    bytes: Vec<u8>,
    /// Number of pages containing review content (before source reference pages).
    pub(crate) ocr_page_count: usize,
}

impl Pdf {
    /// Render review data to a PDF
    pub(crate) fn render(review_data: &ReviewData) -> Result<Self> {
        let world = InkrementWorld::new(review_data)?;

        let warned = compile::<typst::layout::PagedDocument>(&world);

        let document = warned.output.map_err(|diagnostics| {
            let messages: Vec<String> = diagnostics
                .iter()
                .map(|d| format!("{} (hint: {:?})", d.message, d.hints))
                .collect();
            eyre!("typst compilation failed:\n{}", messages.join("\n"))
        })?;

        let options = PdfOptions::default();

        let pdf_bytes = typst_pdf::pdf(&document, &options).map_err(|diagnostics| {
            let messages: Vec<String> = diagnostics.iter().map(|d| d.message.to_string()).collect();
            eyre!("PDF generation failed:\n{}", messages.join("\n"))
        })?;

        // Find the separator page by querying the compiled document's headings.
        // The "Reference Source Code" heading marks where source pages begin.
        let ocr_page_count = Self::find_separator_page(&document)
            .wrap_err("failed to determine where source pages begin")?;

        Ok(Self {
            bytes: pdf_bytes,
            ocr_page_count,
        })
    }

    /// Find the page number of the "Reference Source Code" separator heading.
    /// Returns the number of pages before it (i.e. OCR-relevant page count).
    fn find_separator_page(document: &PagedDocument) -> Result<usize> {
        let selector = Selector::Elem(HeadingElem::ELEM, None);
        let headings = document.introspector.query(&selector);

        for heading in &headings {
            // Check if this heading's body contains our separator text
            let plain = heading.plain_text();
            if plain.contains("Reference Source Code") {
                let location = heading
                    .location()
                    .ok_or_else(|| eyre!("separator heading has no location"))?;
                let page = document.introspector.page(location);
                // page is 1-based; we want the count of pages before it
                return Ok(page.get() - 1);
            }
        }

        Err(eyre!(
            "could not find 'Reference Source Code' heading in compiled document"
        ))
    }

    /// Get the raw PDF bytes
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Write the PDF to a file in the given directory
    pub(crate) fn write(&self, output_dir: &Path, filename: &str) -> Result<()> {
        let path = output_dir.join(filename);
        std::fs::write(&path, &self.bytes)
            .wrap_err_with(|| format!("failed to write {}", path.display()))
    }
}

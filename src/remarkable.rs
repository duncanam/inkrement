use color_eyre::eyre::{Context, Result, eyre};
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::{pdf::Pdf, pull_changes::INKREMENT_TAG};

const BASE_URL: &str = "http://10.11.99.1";

/// A unique identifier for a document or folder on the reMarkable
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct DocumentId(String);

impl DocumentId {
    /// The root folder (empty string parent)
    fn root() -> Self {
        Self(String::new())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

/// The type of entry on the reMarkable
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) enum EntryType {
    CollectionType,
    DocumentType,
}

/// The file format of a document
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum FileType {
    Pdf,
    Epub,
    #[serde(other)]
    Unknown,
}

/// A document or folder on the reMarkable
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct Document {
    #[serde(rename = "ID")]
    pub id: DocumentId,
    #[serde(rename = "Type")]
    pub entry_type: EntryType,
    pub visible_name: String,
    pub parent: DocumentId,
    #[serde(default, rename = "fileType")]
    pub file_type: Option<FileType>,
}

impl Document {
    pub(crate) fn is_folder(&self) -> bool {
        self.entry_type == EntryType::CollectionType
    }

    pub(crate) fn is_document(&self) -> bool {
        self.entry_type == EntryType::DocumentType
    }

    /// Check if this document was created by inkrement
    pub(crate) fn is_inkrement(&self) -> bool {
        self.visible_name.contains(INKREMENT_TAG)
    }
}

/// Client for the reMarkable USB web interface
pub(crate) struct RemarkableClient {
    client: Client,
}

impl RemarkableClient {
    /// Create a new client, verifying the reMarkable is reachable
    pub(crate) fn connect() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .wrap_err("failed to create HTTP client")?;

        // Verify connectivity
        client
            .get(format!("{BASE_URL}/documents/"))
            .send()
            .wrap_err("could not connect to reMarkable — is it plugged in via USB with the web interface enabled?")?;

        Ok(Self { client })
    }

    /// List all documents in a folder (use `DocumentId::root()` for root)
    pub(crate) fn list_documents(&self, parent: &DocumentId) -> Result<Vec<Document>> {
        let url = if parent.as_str().is_empty() {
            format!("{BASE_URL}/documents/")
        } else {
            format!("{BASE_URL}/documents/{}", parent.as_str())
        };

        self.client
            .get(&url)
            .send()
            .wrap_err("failed to list documents on reMarkable")?
            .json::<Vec<Document>>()
            .wrap_err("failed to parse document listing from reMarkable")
    }

    /// List all inkrement documents on the reMarkable (searches root)
    pub(crate) fn list_inkrement_documents(&self) -> Result<Vec<Document>> {
        let docs = self.list_documents(&DocumentId::root())?;
        Ok(docs.into_iter().filter(|d| d.is_inkrement()).collect())
    }

    /// Upload a PDF to the reMarkable (lands in root)
    pub(crate) fn upload(&self, filename: &str, pdf: &Pdf) -> Result<()> {
        let form = reqwest::blocking::multipart::Form::new().part(
            "file",
            reqwest::blocking::multipart::Part::bytes(pdf.as_bytes().to_vec())
                .file_name(filename.to_string())
                .mime_str("application/pdf")
                .wrap_err("failed to set MIME type")?,
        );

        let response = self
            .client
            .post(format!("{BASE_URL}/upload"))
            .multipart(form)
            .send()
            .wrap_err("failed to upload PDF to reMarkable")?;

        if !response.status().is_success() {
            let body = response.text().unwrap_or_default();
            return Err(eyre!("reMarkable upload failed: {body}"));
        }

        Ok(())
    }

    /// Download an annotated PDF from the reMarkable
    pub(crate) fn download(&self, doc_id: &DocumentId) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(format!("{BASE_URL}/download/{}/placeholder", doc_id.as_str()))
            .send()
            .wrap_err("failed to download document from reMarkable")?;

        if !response.status().is_success() {
            return Err(eyre!(
                "reMarkable download failed with status {}",
                response.status()
            ));
        }

        response
            .bytes()
            .map(|b| b.to_vec())
            .wrap_err("failed to read document bytes from reMarkable")
    }
}

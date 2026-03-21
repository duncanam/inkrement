use color_eyre::eyre::{Context, Result, eyre};
use reqwest::blocking::{
    Client,
    multipart::{Form, Part},
};
use serde::Deserialize;

use crate::{pdf::Pdf, pull_changes::INKREMENT_TAG};

const BASE_URL: &str = "http://10.11.99.1";

/// A unique identifier for a document or folder on the reMarkable
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct DocumentId(String);

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
enum EntryType {
    CollectionType,
    DocumentType,
}

/// The file format of a document
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum FileType {
    Pdf,
    Epub,
    #[serde(other)]
    Unknown,
}

/// A document or folder on the reMarkable
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Document {
    #[serde(rename = "ID")]
    id: DocumentId,
    #[serde(rename = "Type")]
    entry_type: EntryType,
    visible_name: String,
    parent: DocumentId,
    #[serde(default, rename = "fileType")]
    file_type: Option<FileType>,
}

impl Document {
    fn is_folder(&self) -> bool {
        self.entry_type == EntryType::CollectionType
    }

    fn is_document(&self) -> bool {
        self.entry_type == EntryType::DocumentType
    }

    /// Check if this document was created by inkrement
    fn is_inkrement(&self) -> bool {
        self.visible_name.contains(INKREMENT_TAG)
    }
}

/// Client for the reMarkable USB web interface
struct RemarkableClient(Client);

impl RemarkableClient {
    /// Create a new client, verifying the reMarkable is reachable
    fn connect() -> Result<Self> {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .wrap_err("failed to create HTTP client while trying to connect to reMarkable")?;

        // Verify connectivity
        client
            .get(format!("{BASE_URL}/documents/"))
            .send()
            .wrap_err("could not connect to reMarkable; is it plugged in via USB with the web interface enabled? Is a VPN blocking LAN connection?")?;

        Ok(Self(client))
    }

    /// List all documents in a folder (use `DocumentId::root()` for root)
    fn list_documents(&self, parent: &DocumentId) -> Result<Box<[Document]>> {
        let url = if parent.as_str().is_empty() {
            format!("{BASE_URL}/documents/")
        } else {
            format!("{BASE_URL}/documents/{}", parent.as_str())
        };

        self.0
            .get(&url)
            .send()
            .wrap_err("failed to list documents on reMarkable")?
            .json()
            .wrap_err("failed to parse document listing from reMarkable")
    }

    /// Fetch all inkcrement_documents on the reMarkable, recursing into all folders
    fn list_all_inkcrement_documents(&self) -> Result<Box<[Document]>> {
        // Prefer manual stack management over recursion
        let mut all = Vec::new();
        let mut stack = vec![DocumentId::root()];

        while let Some(parent) = stack.pop() {
            for doc in self.list_documents(&parent)? {
                if doc.is_folder() {
                    stack.push(doc.id.clone());
                }
                if doc.is_inkrement() {
                    all.push(doc);
                }
            }
        }

        // Generally prefer boxed slices over vecs due to their immutability guarantees and more
        // efficient layouts
        Ok(all.into_boxed_slice())
    }

    /// Upload a PDF to the reMarkable (lands in root)
    fn upload(&self, filename: &str, pdf: &Pdf) -> Result<()> {
        let part = Part::bytes(pdf.as_bytes().to_vec())
            .file_name(filename.to_string())
            .mime_str("application/pdf")
            .wrap_err("failed to set MIME type")?;

        let form = Form::new().part("file", part);

        let response = self
            .0
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
    fn download(&self, doc_id: &DocumentId) -> Result<Vec<u8>> {
        let response = self
            .0
            .get(format!(
                "{BASE_URL}/download/{}/placeholder",
                doc_id.as_str()
            ))
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

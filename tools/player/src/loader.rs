//! Fetches LDraw files over HTTP.

use async_trait::async_trait;
use ldraw::{
    PartAlias,
    color::ColorCatalog,
    document::MultipartDocument,
    error::ResolutionError,
    library::{FileLocation, LibraryLoader, PartKind},
    parser::{parse_color_definitions, parse_multipart_document},
};
use reqwest::{Client, StatusCode, Url};
use tokio::io::BufReader;

/// Response header a resolver sets to say where it found a file: `parts`, `p` or `models`.
pub const FOLDER_HEADER: &str = "x-ldraw-folder";

/// Loads the parts library and a model's own files.
///
/// Library files: with a `resolver` URL, one request each; the server looks in
/// parts/, p/ (and models/) and names the folder in [`FOLDER_HEADER`]. Without
/// one, `library`/parts/ and then `library`/p/ are tried in turn.
///
/// A reference that isn't in the library is looked up next to the model
/// (`document`), for models split over several .ldr files.
pub struct WebLoader {
    client: Client,
    library: Url,
    resolver: Option<Url>,
    document: Option<Url>,
}

impl WebLoader {
    pub fn new(library: Url, resolver: Option<Url>, document: Option<Url>) -> Self {
        Self {
            client: Client::new(),
            library,
            resolver,
            document,
        }
    }

    /// The body (and folder header) of a 200 response; `None` for anything else.
    async fn get(&self, url: Url) -> Result<Option<(Option<String>, Vec<u8>)>, ResolutionError> {
        let response = self.client.get(url).send().await?;
        if response.status() != StatusCode::OK {
            return Ok(None);
        }
        let folder = response
            .headers()
            .get(FOLDER_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        Ok(Some((folder, response.bytes().await?.to_vec())))
    }

    async fn find_in_library(
        &self,
        alias: &PartAlias,
    ) -> Result<Option<(PartKind, Vec<u8>)>, ResolutionError> {
        if let Some(resolver) = &self.resolver {
            let Ok(url) = resolver.join(&alias.normalized) else {
                return Ok(None);
            };
            return Ok(self.get(url).await?.map(|(folder, bytes)| {
                let kind = match folder.as_deref() {
                    Some("p") => PartKind::Primitive,
                    _ => PartKind::Part,
                };
                (kind, bytes)
            }));
        }
        for (folder, kind) in [("parts/", PartKind::Part), ("p/", PartKind::Primitive)] {
            let Ok(url) = self.library.join(&format!("{folder}{}", alias.normalized)) else {
                continue;
            };
            if let Some((_, bytes)) = self.get(url).await? {
                return Ok(Some((kind, bytes)));
            }
        }
        Ok(None)
    }
}

#[async_trait(?Send)]
impl LibraryLoader for WebLoader {
    async fn load_colors(&self) -> Result<ColorCatalog, ResolutionError> {
        let url = self
            .library
            .join("LDConfig.ldr")
            .map_err(|_| ResolutionError::NoLDrawDir)?;
        let Some((_, bytes)) = self.get(url).await? else {
            return Err(ResolutionError::FileNotFound);
        };
        Ok(parse_color_definitions(&mut BufReader::new(&*bytes)).await?)
    }

    async fn load_ref(
        &self,
        alias: PartAlias,
        local: bool,
        colors: &ColorCatalog,
    ) -> Result<(FileLocation, MultipartDocument), ResolutionError> {
        let found = match self.find_in_library(&alias).await? {
            Some((kind, bytes)) => Some((FileLocation::Library(kind), bytes)),
            None => match (&self.document, local) {
                (Some(document), true) => match document.join(&alias.original.replace('\\', "/")) {
                    Ok(url) => self
                        .get(url)
                        .await?
                        .map(|(_, bytes)| (FileLocation::Local, bytes)),
                    Err(_) => None,
                },
                _ => None,
            },
        };
        let (location, bytes) = found.ok_or(ResolutionError::FileNotFound)?;
        Ok((
            location,
            parse_multipart_document(&mut BufReader::new(&*bytes), colors).await?,
        ))
    }
}

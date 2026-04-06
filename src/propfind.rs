//! WebDAV PROPFIND XML response parser.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::CloudError;

/// A parsed entry from a WebDAV PROPFIND response.
#[derive(Debug)]
pub(crate) struct DavEntry {
    pub name: String,
    pub is_collection: bool,
    pub size: u64,
    pub last_modified: String,
    pub content_type: String,
}

/// Parse a WebDAV multistatus XML response.
///
/// Skips the first entry (which is the directory itself).
pub(crate) fn parse_propfind_xml(xml: &str) -> Result<Vec<DavEntry>, CloudError> {
    let mut reader = Reader::from_str(xml);
    let mut entries: Vec<DavEntry> = Vec::new();

    let mut in_response = false;
    let mut in_href = false;
    let mut in_displayname = false;
    let mut in_contentlength = false;
    let mut in_lastmodified = false;
    let mut in_resourcetype = false;
    let mut in_contenttype = false;

    let mut current_href = String::new();
    let mut current_displayname = String::new();
    let mut current_size: u64 = 0;
    let mut current_lastmod = String::new();
    let mut current_is_collection = false;
    let mut current_content_type = String::new();

    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                match local {
                    "response" => {
                        in_response = true;
                        current_href.clear();
                        current_displayname.clear();
                        current_size = 0;
                        current_lastmod.clear();
                        current_is_collection = false;
                        current_content_type.clear();
                    }
                    "href" if in_response => in_href = true,
                    "displayname" if in_response => in_displayname = true,
                    "getcontentlength" if in_response => in_contentlength = true,
                    "getlastmodified" if in_response => in_lastmodified = true,
                    "resourcetype" if in_response => in_resourcetype = true,
                    "getcontenttype" if in_response => in_contenttype = true,
                    "collection" if in_resourcetype => current_is_collection = true,
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let qname = e.name();
                let local = local_name(qname.as_ref());
                match local {
                    "response" => {
                        in_response = false;
                        let name = if current_displayname.is_empty() {
                            name_from_href(&current_href)
                        } else {
                            current_displayname.clone()
                        };

                        if !name.is_empty() {
                            entries.push(DavEntry {
                                name,
                                is_collection: current_is_collection,
                                size: current_size,
                                last_modified: current_lastmod.clone(),
                                content_type: current_content_type.clone(),
                            });
                        }
                    }
                    "href" => in_href = false,
                    "displayname" => in_displayname = false,
                    "getcontentlength" => in_contentlength = false,
                    "getlastmodified" => in_lastmodified = false,
                    "resourcetype" => in_resourcetype = false,
                    "getcontenttype" => in_contenttype = false,
                    _ => {}
                }
            }
            Ok(Event::Text(e)) => {
                if let Ok(text) = e.unescape() {
                    if in_href {
                        current_href.push_str(&text);
                    } else if in_displayname {
                        current_displayname.push_str(&text);
                    } else if in_contentlength {
                        current_size = text.parse().unwrap_or(0);
                    } else if in_lastmodified {
                        current_lastmod.push_str(&text);
                    } else if in_contenttype {
                        current_content_type.push_str(&text);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(CloudError::Nextcloud(format!("XML parse error: {e}")));
            }
            _ => {}
        }
        buf.clear();
    }

    // Skip the first entry (the directory itself)
    if !entries.is_empty() {
        entries.remove(0);
    }

    Ok(entries)
}

/// Extract local name from a possibly namespaced XML tag (`d:href` → `href`).
fn local_name(raw: &[u8]) -> &str {
    let s = std::str::from_utf8(raw).unwrap_or("");
    s.rsplit_once(':').map_or(s, |(_, local)| local)
}

/// Extract a display name from a WebDAV href path.
fn name_from_href(href: &str) -> String {
    let trimmed = href.trim_end_matches('/');
    let segment = trimmed.rsplit('/').next().unwrap_or("");
    urlencoding::decode(segment)
        .map(|s| s.into_owned())
        .unwrap_or_else(|_| segment.to_string())
}

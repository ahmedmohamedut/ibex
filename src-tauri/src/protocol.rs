//! The `ibex://` URI scheme, which streams file bytes to the webview.
//!
//! ```text
//! ibex://localhost/file/<handle>/<fileId>
//! ibex://localhost/thumb/<handle>/<fileId>?s=<maxEdge>
//! ```
//!
//! Media must never travel through `invoke`: a video would be base64-encoded
//! into a JSON message. It also has to support HTTP Range, or `<audio>` and
//! `<video>` cannot seek.
//!
//! `thumb/` answers with a displayable version of a file — a web-safe image
//! passed through untouched, a HEIC/HEIF re-encoded to JPEG at `maxEdge` —
//! via [`ibex_core::thumbs::render`]. `s` defaults to full size when absent.

use std::io::{Read, Seek, SeekFrom};

use tauri::http::{Request, Response, StatusCode, header};

use crate::state::AppState;

/// Largest body served in one response when no Range was requested.
///
/// A webview asking for a whole 4 GB video without a Range header would
/// otherwise be answered with 4 GB in memory. Past this we serve the first
/// chunk as a 206, which makes the player ask for the rest by range.
const MAX_FULL_BODY: u64 = 16 * 1024 * 1024;

/// One parsed byte range, resolved against a known length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Resolved {
    start: u64,
    /// Inclusive.
    end: u64,
}

impl Resolved {
    fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

/// What a `Range` header asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeRequest {
    /// No `Range` header.
    None,
    /// A single satisfiable range.
    Single(Resolved),
    /// Syntactically fine but outside the file.
    Unsatisfiable,
    /// Something we choose not to honour (multiple ranges, bad syntax);
    /// answered with the full body, which is a valid response to any Range.
    Ignored,
}

/// Parse a `Range` header value against the file length.
///
/// Handles `bytes=a-b`, `bytes=a-` and `bytes=-n`. Multiple ranges are ignored
/// rather than refused: a 200 with the whole body is always a legal answer.
fn parse_range(value: Option<&str>, total: u64) -> RangeRequest {
    let Some(value) = value else {
        return RangeRequest::None;
    };
    let Some(spec) = value.trim().strip_prefix("bytes=") else {
        return RangeRequest::Ignored;
    };
    if spec.contains(',') {
        return RangeRequest::Ignored;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return RangeRequest::Ignored;
    };
    let (first, last) = (first.trim(), last.trim());

    // An empty file can satisfy no range at all.
    if total == 0 {
        return RangeRequest::Unsatisfiable;
    }

    match (first.is_empty(), last.is_empty()) {
        // "-n": the final n bytes.
        (true, false) => {
            let Ok(suffix) = last.parse::<u64>() else {
                return RangeRequest::Ignored;
            };
            if suffix == 0 {
                return RangeRequest::Unsatisfiable;
            }
            let start = total.saturating_sub(suffix);
            RangeRequest::Single(Resolved {
                start,
                end: total - 1,
            })
        }
        // "a-": from a to the end.
        (false, true) => {
            let Ok(start) = first.parse::<u64>() else {
                return RangeRequest::Ignored;
            };
            if start >= total {
                return RangeRequest::Unsatisfiable;
            }
            RangeRequest::Single(Resolved {
                start,
                end: total - 1,
            })
        }
        // "a-b".
        (false, false) => {
            let (Ok(start), Ok(end)) = (first.parse::<u64>(), last.parse::<u64>()) else {
                return RangeRequest::Ignored;
            };
            if start > end || start >= total {
                return RangeRequest::Unsatisfiable;
            }
            // An end past the file is clamped, per RFC 9110.
            RangeRequest::Single(Resolved {
                start,
                end: end.min(total - 1),
            })
        }
        (true, true) => RangeRequest::Ignored,
    }
}

/// `/file/<handle>/<fileId>` split out of the URI path.
#[derive(Debug, PartialEq, Eq)]
enum Route<'a> {
    File { handle: &'a str, file_id: &'a str },
    Thumb { handle: &'a str, file_id: &'a str },
    Unknown,
}

fn route(path: &str) -> Route<'_> {
    let mut parts = path.trim_start_matches('/').splitn(3, '/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some("file"), Some(handle), Some(file_id))
            if !handle.is_empty() && !file_id.is_empty() =>
        {
            Route::File { handle, file_id }
        }
        (Some("thumb"), Some(handle), Some(file_id))
            if !handle.is_empty() && !file_id.is_empty() =>
        {
            Route::Thumb { handle, file_id }
        }
        _ => Route::Unknown,
    }
}

/// The `s` query parameter: the requested long edge, or `0` for full size.
///
/// Parsed by hand rather than pulling in a query-string crate for one
/// optional integer; an absent or malformed value just means full size.
fn max_edge_param(request: &Request<Vec<u8>>) -> u32 {
    request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("s=")))
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// A short error response. The body is plain text for easier debugging in the
/// webview's network panel; it carries no file content.
fn fail(status: StatusCode, message: &str) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(message.as_bytes().to_vec())
        .unwrap_or_else(|_| Response::new(Vec::new()))
}

/// Build the response for one request.
///
/// Pure apart from reading the file, so every status and header path is
/// testable without a running webview.
pub fn respond(state: &AppState, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != tauri::http::Method::GET {
        return fail(StatusCode::METHOD_NOT_ALLOWED, "only GET is supported");
    }

    match route(request.uri().path()) {
        Route::File { handle, file_id } => respond_file(state, handle, file_id, request),
        Route::Thumb { handle, file_id } => respond_thumb(state, handle, file_id, request),
        Route::Unknown => fail(StatusCode::NOT_FOUND, "unknown ibex:// route"),
    }
}

fn respond_file(
    state: &AppState,
    handle: &str,
    file_id: &str,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let Ok(backup) = state.get(handle) else {
        return fail(StatusCode::NOT_FOUND, "that backup is not open");
    };
    let entry = match backup.get(file_id) {
        Ok(Some(entry)) => entry,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "no such file in this backup"),
        Err(_) => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the manifest",
            );
        }
    };

    // Resolve to a real path; absent data is a 404, not an error.
    let path = match backup.materialize(&entry) {
        Ok(path) => path,
        Err(_) => return fail(StatusCode::NOT_FOUND, "that file is not in the backup"),
    };

    // Sniff from a prefix so the Content-Type is right even though the file on
    // disk has a hashed, extensionless name.
    let content_type = backup
        .sniff(&entry)
        .map(|s| s.mime)
        .unwrap_or_else(|_| "application/octet-stream".to_string());

    serve_path(&path, content_type, request)
}

/// `thumb/<handle>/<fileId>?s=<maxEdge>`: a version of the file the webview
/// can actually display. See [`ibex_core::thumbs::render`] for what that
/// means for each file type.
fn respond_thumb(
    state: &AppState,
    handle: &str,
    file_id: &str,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let Ok(backup) = state.get(handle) else {
        return fail(StatusCode::NOT_FOUND, "that backup is not open");
    };
    let entry = match backup.get(file_id) {
        Ok(Some(entry)) => entry,
        Ok(None) => return fail(StatusCode::NOT_FOUND, "no such file in this backup"),
        Err(_) => {
            return fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could not read the manifest",
            );
        }
    };

    let max_edge = max_edge_param(request);
    let render = match ibex_core::thumbs::render(&backup, &entry, max_edge) {
        Ok(render) => render,
        // Absent data and an undecodable file are both "no thumbnail" to the
        // caller; neither message carries file content.
        Err(_) => return fail(StatusCode::NOT_FOUND, "could not render a thumbnail"),
    };

    serve_path(&render.path, render.mime, request)
}

/// Serve a file already resolved to a real path, with Range support. Shared
/// by the `file` and `thumb` routes so the status/header matrix is tested
/// once rather than twice.
fn serve_path(
    path: &std::path::Path,
    content_type: String,
    request: &Request<Vec<u8>>,
) -> Response<Vec<u8>> {
    let Ok(metadata) = std::fs::metadata(path) else {
        return fail(StatusCode::NOT_FOUND, "that file is not in the backup");
    };
    let total = metadata.len();

    let header_value = request
        .headers()
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok());

    match parse_range(header_value, total) {
        RangeRequest::Unsatisfiable => Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(header::CONTENT_RANGE, format!("bytes */{total}"))
            .header(header::ACCEPT_RANGES, "bytes")
            .body(Vec::new())
            .unwrap_or_else(|_| Response::new(Vec::new())),

        RangeRequest::Single(range) => match read_at(path, range.start, range.len()) {
            Ok(body) => Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CONTENT_LENGTH, body.len().to_string())
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", range.start, range.end, total),
                )
                .body(body)
                .unwrap_or_else(|_| Response::new(Vec::new())),
            Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file"),
        },

        // No Range, or one we chose not to honour.
        RangeRequest::None | RangeRequest::Ignored => {
            if total > MAX_FULL_BODY {
                // Serve the first chunk as a 206 so the player ranges for the
                // rest instead of us holding the whole file in memory.
                let end = MAX_FULL_BODY - 1;
                return match read_at(path, 0, MAX_FULL_BODY) {
                    Ok(body) => Response::builder()
                        .status(StatusCode::PARTIAL_CONTENT)
                        .header(header::CONTENT_TYPE, content_type)
                        .header(header::ACCEPT_RANGES, "bytes")
                        .header(header::CONTENT_LENGTH, body.len().to_string())
                        .header(header::CONTENT_RANGE, format!("bytes 0-{end}/{total}"))
                        .body(body)
                        .unwrap_or_else(|_| Response::new(Vec::new())),
                    Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file"),
                };
            }
            match std::fs::read(path) {
                Ok(body) => Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, content_type)
                    .header(header::ACCEPT_RANGES, "bytes")
                    .header(header::CONTENT_LENGTH, body.len().to_string())
                    .body(body)
                    .unwrap_or_else(|_| Response::new(Vec::new())),
                Err(_) => fail(StatusCode::INTERNAL_SERVER_ERROR, "could not read the file"),
            }
        }
    }
}

/// Read `len` bytes starting at `offset`, without loading the whole file.
fn read_at(path: &std::path::Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut body = Vec::new();
    file.take(len).read_to_end(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ibex_core::testutil::FixtureBackup;
    use ibex_core::{Backup, CacheDir};

    fn get(uri: &str) -> Request<Vec<u8>> {
        Request::builder().uri(uri).body(Vec::new()).unwrap()
    }

    fn ranged(uri: &str, range: &str) -> Request<Vec<u8>> {
        Request::builder()
            .uri(uri)
            .header(header::RANGE, range)
            .body(Vec::new())
            .unwrap()
    }

    /// A state holding one fixture backup, plus the ids to address it.
    struct Fixture {
        _tmp: tempfile::TempDir,
        state: AppState,
        handle: String,
        wav_id: String,
        text_id: String,
        png_id: String,
        /// `None` on a system whose libheif has no AV1 encoder plugin; every
        /// test using it must handle that rather than failing outright.
        avif_id: Option<String>,
        phantom_id: String,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let mut builder = FixtureBackup::new().with_sample_media().with_phantom_file(
            "HomeDomain",
            "Library/gone.bin",
            999,
        );
        let has_avif = ibex_core::testutil::photos::avif(64, 48).is_some();
        if let Some(bytes) = ibex_core::testutil::photos::avif(64, 48) {
            builder = builder.with_file("HomeDomain", "Library/IbexSamples/photo.heic", &bytes);
        }
        let root = builder.build(tmp.path()).unwrap();
        let backup =
            Backup::open_with_cache(&root, None, CacheDir::at(tmp.path().join("cache"))).unwrap();

        let id = |path: &str| {
            backup
                .find("HomeDomain", path)
                .unwrap()
                .unwrap_or_else(|| panic!("{path} missing"))
                .file_id
        };
        let wav_id = id("Library/IbexSamples/tone.wav");
        let text_id = id("Library/IbexSamples/notes.txt");
        let png_id = id("Library/IbexSamples/pixel.png");
        let avif_id = has_avif.then(|| id("Library/IbexSamples/photo.heic"));
        let phantom_id = id("Library/gone.bin");

        let state = AppState::new();
        let handle = state.insert(backup);
        Fixture {
            _tmp: tmp,
            state,
            handle,
            wav_id,
            text_id,
            png_id,
            avif_id,
            phantom_id,
        }
    }

    #[test]
    fn range_parsing_covers_the_three_forms() {
        // Explicit range.
        assert_eq!(
            parse_range(Some("bytes=10-19"), 100),
            RangeRequest::Single(Resolved { start: 10, end: 19 })
        );
        // Open-ended.
        assert_eq!(
            parse_range(Some("bytes=90-"), 100),
            RangeRequest::Single(Resolved { start: 90, end: 99 })
        );
        // Suffix.
        assert_eq!(
            parse_range(Some("bytes=-10"), 100),
            RangeRequest::Single(Resolved { start: 90, end: 99 })
        );
        // An end past the file is clamped rather than refused.
        assert_eq!(
            parse_range(Some("bytes=95-500"), 100),
            RangeRequest::Single(Resolved { start: 95, end: 99 })
        );
        // A suffix longer than the file yields the whole file.
        assert_eq!(
            parse_range(Some("bytes=-500"), 100),
            RangeRequest::Single(Resolved { start: 0, end: 99 })
        );
        assert_eq!(parse_range(None, 100), RangeRequest::None);
    }

    #[test]
    fn range_parsing_rejects_what_it_cannot_serve() {
        assert_eq!(
            parse_range(Some("bytes=100-"), 100),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=50-20"), 100),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=-0"), 100),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=0-0"), 0),
            RangeRequest::Unsatisfiable
        );
        // Multiple ranges and nonsense fall back to the full body.
        assert_eq!(
            parse_range(Some("bytes=0-1,5-6"), 100),
            RangeRequest::Ignored
        );
        assert_eq!(parse_range(Some("items=0-1"), 100), RangeRequest::Ignored);
        assert_eq!(
            parse_range(Some("bytes=abc-def"), 100),
            RangeRequest::Ignored
        );
        assert_eq!(parse_range(Some("bytes=-"), 100), RangeRequest::Ignored);
    }

    #[test]
    fn routing() {
        assert_eq!(
            route("/file/abc/def"),
            Route::File {
                handle: "abc",
                file_id: "def"
            }
        );
        assert_eq!(
            route("/thumb/abc/def"),
            Route::Thumb {
                handle: "abc",
                file_id: "def"
            }
        );
        assert_eq!(route("/file/abc"), Route::Unknown);
        assert_eq!(route("/file//def"), Route::Unknown);
        assert_eq!(route("/thumb/abc"), Route::Unknown);
        assert_eq!(route("/"), Route::Unknown);
        assert_eq!(route("/nonsense/a/b"), Route::Unknown);
    }

    #[test]
    fn the_max_edge_query_parameter_is_parsed_leniently() {
        assert_eq!(max_edge_param(&get("ibex://localhost/thumb/a/b")), 0);
        assert_eq!(
            max_edge_param(&get("ibex://localhost/thumb/a/b?s=512")),
            512
        );
        assert_eq!(
            max_edge_param(&get("ibex://localhost/thumb/a/b?x=1&s=256")),
            256
        );
        // Malformed values just mean full size, not an error.
        assert_eq!(max_edge_param(&get("ibex://localhost/thumb/a/b?s=abc")), 0);
    }

    #[test]
    fn a_whole_small_file_comes_back_200_with_its_type() {
        let f = fixture();
        let res = respond(
            &f.state,
            &get(&format!("ibex://localhost/file/{}/{}", f.handle, f.text_id)),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain"
        );
        assert_eq!(res.headers().get(header::ACCEPT_RANGES).unwrap(), "bytes");
        assert!(res.body().starts_with(b"ibex synthetic fixture"));
        assert_eq!(
            res.headers()
                .get(header::CONTENT_LENGTH)
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<usize>()
                .unwrap(),
            res.body().len(),
            "Content-Length must match the body"
        );
    }

    #[test]
    fn the_wav_is_served_as_audio_so_the_player_accepts_it() {
        let f = fixture();
        let res = respond(
            &f.state,
            &get(&format!("ibex://localhost/file/{}/{}", f.handle, f.wav_id)),
        );
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(
            ct.to_str().unwrap().starts_with("audio/"),
            "got {ct:?}; a non-audio type stops <audio> from playing it"
        );
    }

    #[test]
    fn a_range_request_comes_back_206_with_exactly_those_bytes() {
        let f = fixture();
        let uri = format!("ibex://localhost/file/{}/{}", f.handle, f.wav_id);

        // The WAV is 5 seconds at 8 kHz plus a 44-byte header.
        let full = respond(&f.state, &get(&uri));
        let total = full.body().len() as u64;
        assert_eq!(total, 44 + 8_000 * 5);

        let res = respond(&f.state, &ranged(&uri, "bytes=100-199"));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), 100);
        assert_eq!(
            res.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes 100-199/{total}")
        );
        // And the bytes are the right ones.
        assert_eq!(res.body(), &full.body()[100..200]);
    }

    #[test]
    fn seeking_to_the_tail_works() {
        // This is the shape a media player uses to find a file's duration.
        let f = fixture();
        let uri = format!("ibex://localhost/file/{}/{}", f.handle, f.wav_id);
        let full = respond(&f.state, &get(&uri));
        let total = full.body().len() as u64;

        let res = respond(&f.state, &ranged(&uri, "bytes=-64"));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len(), 64);
        assert_eq!(
            res.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes {}-{}/{}", total - 64, total - 1, total)
        );
        assert_eq!(res.body(), &full.body()[(total as usize - 64)..]);

        // Open-ended from the middle, as a scrub does.
        let res = respond(&f.state, &ranged(&uri, "bytes=20000-"));
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(res.body().len() as u64, total - 20_000);
    }

    #[test]
    fn an_impossible_range_is_416_with_the_real_length() {
        let f = fixture();
        let uri = format!("ibex://localhost/file/{}/{}", f.handle, f.text_id);
        let total = respond(&f.state, &get(&uri)).body().len();

        let res = respond(&f.state, &ranged(&uri, "bytes=99999-"));
        assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            res.headers().get(header::CONTENT_RANGE).unwrap(),
            &format!("bytes */{total}"),
            "a 416 must tell the client the real length"
        );
        assert!(res.body().is_empty());
    }

    #[test]
    fn multiple_ranges_fall_back_to_the_whole_body() {
        let f = fixture();
        let uri = format!("ibex://localhost/file/{}/{}", f.handle, f.text_id);
        let res = respond(&f.state, &ranged(&uri, "bytes=0-4,10-14"));
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "200 answers any Range legally"
        );
    }

    #[test]
    fn unknown_handles_files_and_routes_are_404() {
        let f = fixture();
        for uri in [
            format!("ibex://localhost/file/nosuchhandle/{}", f.text_id),
            format!("ibex://localhost/file/{}/0000000000", f.handle),
            // Separately covered: a thumb request for a *known* file that is
            // not an image still answers 200, passed through unchanged —
            // `a_thumbnail_for_an_unknown_backup_or_file_is_404` covers the
            // actually-404 thumb cases.
            "ibex://localhost/".to_string(),
            format!("ibex://localhost/file/{}", f.handle),
        ] {
            assert_eq!(
                respond(&f.state, &get(&uri)).status(),
                StatusCode::NOT_FOUND,
                "{uri} should be 404"
            );
        }
    }

    #[test]
    fn a_file_listed_but_absent_from_the_backup_is_404() {
        let f = fixture();
        let res = respond(
            &f.state,
            &get(&format!(
                "ibex://localhost/file/{}/{}",
                f.handle, f.phantom_id
            )),
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn non_get_methods_are_refused() {
        let f = fixture();
        let req = Request::builder()
            .method(tauri::http::Method::POST)
            .uri(format!("ibex://localhost/file/{}/{}", f.handle, f.text_id))
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            respond(&f.state, &req).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[test]
    fn a_web_safe_thumbnail_passes_through_untouched() {
        let f = fixture();
        let res = respond(
            &f.state,
            &get(&format!("ibex://localhost/thumb/{}/{}", f.handle, f.png_id)),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/png"
        );
    }

    #[test]
    fn a_heif_thumbnail_is_served_as_jpeg() {
        let f = fixture();
        let Some(avif_id) = &f.avif_id else {
            println!("skipping: this system's libheif has no AV1 encoder plugin");
            return;
        };
        let res = respond(
            &f.state,
            &get(&format!(
                "ibex://localhost/thumb/{}/{}?s=32",
                f.handle, avif_id
            )),
        );
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        assert_eq!(&res.body()[..2], &[0xFF, 0xD8], "the body is not a JPEG");
    }

    #[test]
    fn a_thumbnail_for_absent_data_is_404() {
        let f = fixture();
        let res = respond(
            &f.state,
            &get(&format!(
                "ibex://localhost/thumb/{}/{}",
                f.handle, f.phantom_id
            )),
        );
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn a_thumbnail_for_an_unknown_backup_or_file_is_404() {
        let f = fixture();
        assert_eq!(
            respond(
                &f.state,
                &get(&format!("ibex://localhost/thumb/not-a-handle/{}", f.png_id))
            )
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            respond(
                &f.state,
                &get(&format!("ibex://localhost/thumb/{}/not-a-file", f.handle))
            )
            .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn non_get_is_refused_on_the_thumb_route_too() {
        let f = fixture();
        let req = Request::builder()
            .method(tauri::http::Method::POST)
            .uri(format!("ibex://localhost/thumb/{}/{}", f.handle, f.png_id))
            .body(Vec::new())
            .unwrap();
        assert_eq!(
            respond(&f.state, &req).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }
}

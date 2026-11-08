//! The camera roll, from `Photos.sqlite` in `CameraRollDomain`.
//!
//! Core Data again, so tables are prefixed `Z` and columns `Z`. Planned and
//! written against a real iOS 27.2 library of 7,865 assets in a 440 MiB
//! database; `docs/DATA_SOURCES.md` records that inspection in full. Four of
//! its findings shape this module:
//!
//! - **Every asset already has a thumbnail in the backup.** Apple keeps a
//!   360 px JPEG at `Media/PhotoData/Thumbnails/V2/<dir>/<file>/5005.JPG`, and
//!   the real library had one for all 7,865 assets. The grid therefore decodes
//!   nothing: it loads these over the existing `ibex://` file route.
//! - **`ZUNIFORMTYPEIDENTIFIER` lies.** It claimed `public.jpeg` for 578 assets
//!   whose files are PNG. The media type comes from the file name and the
//!   sniffed bytes, never from that column.
//! - **`ZHIDDEN` was 0 for every row**, while `ZVISIBILITYSTATE` marked 47
//!   assets hidden. Filtering on `ZHIDDEN` would leak hidden photos into the
//!   main grid, so both are consulted and either one hides an asset.
//! - **`ZAVALANCHEUUID` is burst grouping, not Live Photos** (52 rows against
//!   2,302 Live Photos). `ZKINDSUBTYPE`/`ZPLAYBACKSTYLE` identify those, and
//!   the companion video is the same base name with a `.MOV` extension — it is
//!   not a row of its own.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, Utc};
use rusqlite::{Connection, types::Value};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::{ExtractError, Result};
use crate::extractors::{Availability, Extractor};
use crate::util::mime::{self, PreviewKind};
use crate::util::time::from_apple;
use crate::{Backup, ManifestEntry};

/// The domain holding both the library and the files it points at.
pub const DOMAIN: &str = "CameraRollDomain";
/// The library database, relative to [`DOMAIN`].
pub const PHOTOS_DB: &str = "Media/PhotoData/Photos.sqlite";

/// `ZDIRECTORY`/`ZFILENAME` are relative to this.
const MEDIA_PREFIX: &str = "Media/";
/// Apple's pre-rendered derivatives live under here, one directory per asset.
const THUMB_PREFIX: &str = "Media/PhotoData/Thumbnails/V2/";
/// The only derivative name seen in the real library, for photos and videos.
const THUMB_FILE: &str = "5005.JPG";
/// Rendered edits live under here, keyed by the asset's base name.
const MUTATIONS_PREFIX: &str = "Media/PhotoData/Mutations/";
/// Extensions a `FullSizeRender` has been seen with. A PNG original's edit is
/// rendered as `.jpg`, so the original's extension is no guide.
const RENDER_EXTENSIONS: &[&str] = &["heic", "jpg", "jpeg", "png", "HEIC", "JPG"];
/// The companion video of a Live Photo, and of an edited Live Photo.
const LIVE_EXTENSIONS: &[&str] = &["MOV", "mov"];

/// `ZKIND`: 0 is a still, 1 is a video.
const KIND_VIDEO: i64 = 1;
/// `ZKINDSUBTYPE` 2 and `ZPLAYBACKSTYLE` 3 both mark a Live Photo.
const SUBTYPE_LIVE: i64 = 2;
const PLAYBACK_LIVE: i64 = 3;
/// `ZLATITUDE`/`ZLONGITUDE` use this as "no location" rather than NULL.
const NO_COORDINATE: f64 = -180.0;

/// The Photos extractor.
pub struct Photos;

impl Extractor for Photos {
    const ID: &'static str = "photos";

    fn detect(backup: &Backup) -> Availability {
        match backup.find(DOMAIN, PHOTOS_DB) {
            Ok(Some(entry)) if backup.has_data(&entry) => {
                Availability::Present { version_hint: None }
            }
            Ok(Some(_)) => Availability::Unsupported(
                "Photos.sqlite is in the manifest but its contents are not in the backup"
                    .to_string(),
            ),
            // The library can be missing while the files are there; the DCIM
            // fallback handles that, and reports itself separately.
            _ if dcim::has_any(backup) => Availability::Present {
                version_hint: Some("DCIM only (no Photos.sqlite)".to_string()),
            },
            _ => Availability::Absent,
        }
    }
}

/// Still or video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PhotoKind {
    Photo,
    Video,
}

/// Whether the asset's bytes are actually in this backup.
///
/// `CloudOnly` is the "Optimize iPhone Storage" case: the row is in the library
/// but the file was never downloaded to the device. The real backup had none,
/// so this arm is exercised only by the fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PhotoAvailability {
    InBackup,
    CloudOnly,
    /// In the library, not in the backup, and not flagged as cloud-only.
    Missing,
}

/// A capture location, when there is one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GeoPoint {
    pub latitude: f64,
    pub longitude: f64,
}

/// One asset in the library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhotoAsset {
    /// `Z_PK`, as a string so it travels over IPC like every other id.
    pub id: String,
    pub uuid: Option<String>,
    pub kind: PhotoKind,
    /// The original file, when it is in the backup.
    pub file_id: Option<String>,
    /// Apple's 360 px derivative. What the grid renders.
    pub thumb_file_id: Option<String>,
    /// The companion video of a Live Photo.
    pub live_file_id: Option<String>,
    /// Apple's rendered edit, when the asset has been edited.
    pub edited_file_id: Option<String>,
    /// The companion video of an *edited* Live Photo.
    pub edited_live_file_id: Option<String>,
    /// The name on disk, which is also the display name.
    pub file_name: String,
    /// The name the file had when it was imported, when it differs.
    pub original_file_name: Option<String>,
    /// MIME type from the extension, confirmed by sniffing when the bytes are
    /// present. Never from `ZUNIFORMTYPEIDENTIFIER`.
    pub mime: Option<String>,
    pub preview: Option<PreviewKind>,
    pub created_at: Option<DateTime<Utc>>,
    pub added_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub width: u32,
    pub height: u32,
    /// Seconds, for videos.
    pub duration: Option<f64>,
    pub size: Option<u64>,
    pub favorite: bool,
    pub screenshot: bool,
    pub edited: bool,
    pub live: bool,
    pub trashed: bool,
    pub hidden: bool,
    pub location: Option<GeoPoint>,
    pub availability: PhotoAvailability,
}

impl PhotoAsset {
    /// The file to show at full size: the rendered edit if there is one, else
    /// the original. This is what Photos itself displays.
    pub fn display_file_id(&self) -> Option<&str> {
        self.edited_file_id.as_deref().or(self.file_id.as_deref())
    }

    /// The companion video to play, matching [`Self::display_file_id`].
    pub fn display_live_file_id(&self) -> Option<&str> {
        if self.edited_file_id.is_some() {
            self.edited_live_file_id
                .as_deref()
                .or(self.live_file_id.as_deref())
        } else {
            self.live_file_id.as_deref()
        }
    }
}

/// Which assets to list.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PhotoFilter {
    /// `None` lists both stills and videos.
    pub kind: Option<PhotoKind>,
    pub favorites_only: bool,
    pub screenshots_only: bool,
    /// "Recently Deleted". Off by default, and exclusive: on, it shows *only*
    /// trashed assets, because that is what the album means.
    pub trashed_only: bool,
    /// The Hidden album, same exclusive logic.
    pub hidden_only: bool,
    /// Restrict to one album, by `ZGENERICALBUM.Z_PK`.
    pub album_id: Option<String>,
}

/// Keyset cursor over `(ZDATECREATED DESC, Z_PK DESC)`.
///
/// The same shape as [`crate::extractors::MessageCursor`]: an offset would
/// re-scan the whole table for every page of a 7,865-asset grid.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhotoCursor {
    /// `ZDATECREATED` of the last asset on the previous page.
    pub before_date: Option<f64>,
    pub before_pk: Option<i64>,
    pub limit: u32,
}

impl Default for PhotoCursor {
    fn default() -> Self {
        Self {
            before_date: None,
            before_pk: None,
            limit: Self::DEFAULT_LIMIT,
        }
    }
}

impl PhotoCursor {
    pub const DEFAULT_LIMIT: u32 = 200;
    pub const MAX_LIMIT: u32 = 1_000;

    pub fn first(limit: u32) -> Self {
        Self {
            before_date: None,
            before_pk: None,
            limit: limit.clamp(1, Self::MAX_LIMIT),
        }
    }

    pub(crate) fn clamped_limit(&self) -> u32 {
        self.limit.clamp(1, Self::MAX_LIMIT)
    }
}

/// One page of the grid, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PhotoPage {
    pub assets: Vec<PhotoAsset>,
    /// Pass back to load the next page. `None` at the end of the library.
    pub older: Option<PhotoCursor>,
}

/// How many assets fall in one month, for the grid's headers and its jump bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MonthCount {
    pub year: i32,
    /// 1-12.
    pub month: u32,
    pub count: u64,
}

/// A user album.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AlbumSummary {
    pub id: String,
    pub title: String,
    pub count: u64,
}

/// The album membership join table, discovered rather than assumed.
#[derive(Debug, Clone)]
struct AlbumJoin {
    table: String,
    album_column: String,
    asset_column: String,
}

/// An open `Photos.sqlite`.
pub struct PhotosDb<'a> {
    backup: &'a Backup,
    conn: Connection,
    /// `ZASSET` on iOS 14+, `ZGENERICASSET` before it.
    asset_table: &'static str,
    /// Columns `asset_table` actually has, for hard rule 6.
    asset_columns: HashSet<String>,
    album_join: Option<AlbumJoin>,
    /// Whether `ZADDITIONALASSETATTRIBUTES` is present and joinable.
    additional_join: Option<&'static str>,
}

impl<'a> PhotosDb<'a> {
    /// Open the library.
    pub fn open(backup: &'a Backup) -> Result<Self> {
        let entry = backup
            .find(DOMAIN, PHOTOS_DB)
            .ok()
            .flatten()
            .filter(|e| backup.has_data(e))
            .ok_or(ExtractError::SourceAbsent {
                source_id: Photos::ID,
            })?;
        let _ = entry;

        let conn = backup.open_db(DOMAIN, PHOTOS_DB)?;

        let asset_table = if table_exists(&conn, "ZASSET") {
            "ZASSET"
        } else if table_exists(&conn, "ZGENERICASSET") {
            "ZGENERICASSET"
        } else {
            return Err(ExtractError::Unsupported {
                source_id: Photos::ID,
                detail: "neither ZASSET nor ZGENERICASSET is present".to_string(),
            }
            .into());
        };

        let asset_columns = table_columns(&conn, asset_table);
        if !asset_columns.contains("ZFILENAME") || !asset_columns.contains("ZDIRECTORY") {
            return Err(ExtractError::Unsupported {
                source_id: Photos::ID,
                detail: format!("{asset_table} has no ZDIRECTORY/ZFILENAME to resolve files with"),
            }
            .into());
        }

        let album_join = discover_album_join(&conn);
        let additional_join = if table_exists(&conn, "ZADDITIONALASSETATTRIBUTES") {
            let cols = table_columns(&conn, "ZADDITIONALASSETATTRIBUTES");
            if cols.contains("ZORIGINALFILENAME") && cols.contains("ZASSET") {
                Some("ZASSET")
            } else {
                None
            }
        } else {
            None
        };

        Ok(Self {
            backup,
            conn,
            asset_table,
            asset_columns,
            album_join,
            additional_join,
        })
    }

    /// Borrow the connection, for structure-only queries in tests.
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Which asset table this library uses, for diagnostics.
    pub fn asset_table(&self) -> &'static str {
        self.asset_table
    }

    /// The album join table that was discovered, for diagnostics and tests.
    pub fn album_join_table(&self) -> Option<&str> {
        self.album_join.as_ref().map(|j| j.table.as_str())
    }

    /// Whether an optional column is present.
    fn has(&self, column: &str) -> bool {
        self.asset_columns.contains(column)
    }

    /// `a.<column>` when it exists, `NULL` when it does not, so one SQL string
    /// serves every schema variant.
    fn col(&self, column: &str) -> String {
        if self.has(column) {
            format!("a.{column}")
        } else {
            "NULL".to_string()
        }
    }

    /// One page of the grid, newest first.
    pub fn list(&self, filter: &PhotoFilter, cursor: PhotoCursor) -> Result<PhotoPage> {
        let limit = cursor.clamped_limit();
        let (where_sql, mut params) = self.filter_sql(filter);

        // Keyset: strictly older than the last asset of the previous page,
        // with Z_PK breaking ties on identical timestamps.
        let keyset = match (cursor.before_date, cursor.before_pk) {
            (Some(date), Some(pk)) => {
                params.push(Value::Real(date));
                params.push(Value::Real(date));
                params.push(Value::Integer(pk));
                " AND (a.ZDATECREATED < ?a OR (a.ZDATECREATED = ?b AND a.Z_PK < ?c))".to_string()
            }
            // A row with no date sorts last, so a cursor without one can only
            // continue by primary key.
            (None, Some(pk)) => {
                params.push(Value::Integer(pk));
                " AND a.Z_PK < ?c".to_string()
            }
            _ => String::new(),
        };
        // Positional placeholders are rebuilt in order below; the named-looking
        // markers above are only there to keep this readable.
        let keyset = renumber(&keyset, params.len() - keyset.matches('?').count() + 1);

        let sql = format!(
            "SELECT {select} FROM {table} a {joins} WHERE {where_sql}{keyset} \
             ORDER BY a.ZDATECREATED IS NULL, a.ZDATECREATED DESC, a.Z_PK DESC LIMIT {limit}",
            select = self.select_list(),
            table = self.asset_table,
            joins = self.join_clause(filter),
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            AssetRow::from_row(row)
        })?;

        let mut assets = Vec::new();
        let mut last = None;
        for row in rows {
            let row = row?;
            last = Some((row.date, row.pk));
            assets.push(self.hydrate(row));
        }

        let older = if assets.len() as u32 == limit {
            last.map(|(date, pk)| PhotoCursor {
                before_date: date,
                before_pk: Some(pk),
                limit,
            })
        } else {
            None
        };
        Ok(PhotoPage { assets, older })
    }

    /// One asset, for the lightbox.
    pub fn asset(&self, id: &str) -> Result<Option<PhotoAsset>> {
        let pk: i64 = match id.parse() {
            Ok(pk) => pk,
            Err(_) => return Ok(None),
        };
        let sql = format!(
            "SELECT {select} FROM {table} a {joins} WHERE a.Z_PK = ?1",
            select = self.select_list(),
            table = self.asset_table,
            joins = self.join_clause(&PhotoFilter::default()),
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query_map([pk], AssetRow::from_row)?;
        match rows.next() {
            Some(row) => Ok(Some(self.hydrate(row?))),
            None => Ok(None),
        }
    }

    /// How many assets fall in each month, newest month first.
    pub fn timeline(&self, filter: &PhotoFilter) -> Result<Vec<MonthCount>> {
        let (where_sql, params) = self.filter_sql(filter);
        let sql = format!(
            "SELECT a.ZDATECREATED, count(*) FROM {table} a {joins} WHERE {where_sql} \
             GROUP BY strftime('%Y-%m', a.ZDATECREATED + {epoch}, 'unixepoch') \
             ORDER BY a.ZDATECREATED DESC",
            table = self.asset_table,
            joins = self.join_clause(filter),
            epoch = crate::util::time::APPLE_EPOCH_UNIX,
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            Ok((row.get::<_, Option<f64>>(0)?, row.get::<_, i64>(1)?))
        })?;

        let mut out = Vec::new();
        for row in rows {
            let (date, count) = row?;
            // An asset with no date cannot be placed in a month; it still shows
            // in the grid, under its own header in the UI.
            if let Some(when) = date.and_then(from_apple) {
                out.push(MonthCount {
                    year: when.year(),
                    month: when.month(),
                    count: count.max(0) as u64,
                });
            }
        }
        Ok(out)
    }

    /// Titled albums with their asset counts, largest first.
    ///
    /// The real library had 80 album rows but only 11 titles: the rest are
    /// system and smart albums that would be noise in a picker.
    pub fn albums(&self) -> Result<Vec<AlbumSummary>> {
        let Some(join) = &self.album_join else {
            return Ok(Vec::new());
        };
        if !table_exists(&self.conn, "ZGENERICALBUM") {
            return Ok(Vec::new());
        }
        let sql = format!(
            "SELECT g.Z_PK, g.ZTITLE, count(j.{asset}) \
             FROM ZGENERICALBUM g \
             JOIN {table} j ON j.{album} = g.Z_PK \
             JOIN {assets} a ON a.Z_PK = j.{asset} \
             WHERE g.ZTITLE IS NOT NULL AND trim(g.ZTITLE) <> '' \
             GROUP BY g.Z_PK, g.ZTITLE \
             ORDER BY count(j.{asset}) DESC, g.ZTITLE",
            table = join.table,
            album = join.album_column,
            asset = join.asset_column,
            assets = self.asset_table,
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(AlbumSummary {
                id: row.get::<_, i64>(0)?.to_string(),
                title: row.get::<_, String>(1)?,
                count: row.get::<_, i64>(2)?.max(0) as u64,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Total assets matching a filter, for the header and the export estimate.
    pub fn count(&self, filter: &PhotoFilter) -> Result<u64> {
        let (where_sql, params) = self.filter_sql(filter);
        let sql = format!(
            "SELECT count(*) FROM {table} a {joins} WHERE {where_sql}",
            table = self.asset_table,
            joins = self.join_clause(filter),
        );
        let n: i64 = self
            .conn
            .query_one(&sql, rusqlite::params_from_iter(params), |row| row.get(0))?;
        Ok(n.max(0) as u64)
    }

    /// The album join, when a filter needs it.
    fn join_clause(&self, filter: &PhotoFilter) -> String {
        let mut joins = String::new();
        if let Some(name) = self.additional_join {
            joins.push_str(&format!(
                " LEFT JOIN ZADDITIONALASSETATTRIBUTES t ON t.{name} = a.Z_PK"
            ));
        }
        if let (Some(join), Some(_)) = (&self.album_join, filter.album_id.as_deref()) {
            joins.push_str(&format!(
                " JOIN {table} j ON j.{asset} = a.Z_PK",
                table = join.table,
                asset = join.asset_column,
            ));
        }
        joins
    }

    /// The `WHERE` body and its parameters, with `?1` numbering.
    fn filter_sql(&self, filter: &PhotoFilter) -> (String, Vec<Value>) {
        let mut clauses: Vec<String> = Vec::new();
        let mut params: Vec<Value> = Vec::new();

        // Hidden and trashed are exclusive views, not extra rows.
        if filter.trashed_only {
            clauses.push(self.truthy("ZTRASHEDSTATE"));
        } else {
            clauses.push(self.falsy("ZTRASHEDSTATE"));
        }
        if filter.hidden_only {
            clauses.push(self.hidden_clause(true));
        } else {
            clauses.push(self.hidden_clause(false));
        }

        if let Some(kind) = filter.kind {
            let op = if kind == PhotoKind::Video { "=" } else { "<>" };
            if self.has("ZKIND") {
                clauses.push(format!("coalesce(a.ZKIND, 0) {op} {KIND_VIDEO}"));
            }
        }
        if filter.favorites_only {
            clauses.push(self.truthy("ZFAVORITE"));
        }
        if filter.screenshots_only && self.has("ZISDETECTEDSCREENSHOT") {
            clauses.push(self.truthy("ZISDETECTEDSCREENSHOT"));
        }
        if let (Some(join), Some(album)) = (&self.album_join, filter.album_id.as_deref())
            && let Ok(pk) = album.parse::<i64>()
        {
            clauses.push(format!("j.{} = ?", join.album_column));
            params.push(Value::Integer(pk));
        }

        if clauses.is_empty() {
            clauses.push("1 = 1".to_string());
        }
        let where_sql = renumber(&clauses.join(" AND "), 1);
        (where_sql, params)
    }

    /// `coalesce(a.<col>, 0) <> 0`, or always-true when the column is absent.
    fn truthy(&self, column: &str) -> String {
        if self.has(column) {
            format!("coalesce(a.{column}, 0) <> 0")
        } else {
            "1 = 1".to_string()
        }
    }

    fn falsy(&self, column: &str) -> String {
        if self.has(column) {
            format!("coalesce(a.{column}, 0) = 0")
        } else {
            "1 = 1".to_string()
        }
    }

    /// Hidden is two columns, and in the real library only the second one was
    /// ever set: `ZHIDDEN` was 0 for all 7,865 assets while `ZVISIBILITYSTATE`
    /// marked 47 hidden. Either being set hides the asset.
    fn hidden_clause(&self, want_hidden: bool) -> String {
        let mut parts = Vec::new();
        if self.has("ZHIDDEN") {
            parts.push("coalesce(a.ZHIDDEN, 0) <> 0".to_string());
        }
        if self.has("ZVISIBILITYSTATE") {
            parts.push("coalesce(a.ZVISIBILITYSTATE, 0) <> 0".to_string());
        }
        if parts.is_empty() {
            return "1 = 1".to_string();
        }
        let any = parts.join(" OR ");
        if want_hidden {
            format!("({any})")
        } else {
            format!("NOT ({any})")
        }
    }

    /// The column list, in the order [`AssetRow::from_row`] reads it.
    fn select_list(&self) -> String {
        let original_name = if self.additional_join.is_some() {
            "t.ZORIGINALFILENAME"
        } else {
            "NULL"
        };
        let size = if self.additional_join.is_some() {
            "t.ZORIGINALFILESIZE"
        } else {
            "NULL"
        };
        [
            "a.Z_PK".to_string(),
            self.col("ZUUID"),
            "a.ZDIRECTORY".to_string(),
            "a.ZFILENAME".to_string(),
            self.col("ZKIND"),
            self.col("ZDATECREATED"),
            self.col("ZADDEDDATE"),
            self.col("ZMODIFICATIONDATE"),
            self.col("ZWIDTH"),
            self.col("ZHEIGHT"),
            self.col("ZDURATION"),
            self.col("ZFAVORITE"),
            self.col("ZISDETECTEDSCREENSHOT"),
            self.col("ZADJUSTMENTSSTATE"),
            self.col("ZKINDSUBTYPE"),
            self.col("ZPLAYBACKSTYLE"),
            self.col("ZTRASHEDSTATE"),
            self.col("ZHIDDEN"),
            self.col("ZVISIBILITYSTATE"),
            self.col("ZLATITUDE"),
            self.col("ZLONGITUDE"),
            self.col("ZCLOUDLOCALSTATE"),
            self.col("ZCOMPLETE"),
            original_name.to_string(),
            size.to_string(),
        ]
        .join(", ")
    }

    /// Turn a row into an asset, resolving its files against the manifest.
    fn hydrate(&self, row: AssetRow) -> PhotoAsset {
        let relative = format!("{MEDIA_PREFIX}{}/{}", row.directory, row.file_name);
        let original = self.resolve(&relative);

        // Apple's own derivative: one directory per asset, named after the
        // asset's path, holding a single JPEG.
        let thumb = self.resolve(&format!(
            "{THUMB_PREFIX}{}/{}/{THUMB_FILE}",
            row.directory, row.file_name
        ));

        let live = row.is_live().then(|| {
            self.resolve_any(
                LIVE_EXTENSIONS
                    .iter()
                    .map(|ext| replace_extension(&relative, ext)),
            )
        });
        let live = live.flatten();

        let edited = row.is_edited().then(|| {
            let stem = file_stem(&row.file_name);
            self.resolve_any(RENDER_EXTENSIONS.iter().map(|ext| {
                format!(
                    "{MUTATIONS_PREFIX}{}/{stem}/Adjustments/FullSizeRender.{ext}",
                    row.directory
                )
            }))
        });
        let edited = edited.flatten();

        let edited_live = (row.is_edited() && row.is_live())
            .then(|| {
                let stem = file_stem(&row.file_name);
                self.resolve_any(LIVE_EXTENSIONS.iter().map(|ext| {
                    format!(
                        "{MUTATIONS_PREFIX}{}/{stem}/Adjustments/FullSizeRender.{ext}",
                        row.directory
                    )
                }))
            })
            .flatten();

        // Extension first, then the real bytes. ZUNIFORMTYPEIDENTIFIER is
        // deliberately not consulted: it was wrong for 578 real assets.
        let by_extension = mime_guess::from_path(&row.file_name)
            .first_raw()
            .map(str::to_string);
        let sniffed = original.as_ref().and_then(|e| self.backup.sniff(e).ok());
        let mime = sniffed
            .as_ref()
            .map(|s| s.mime.clone())
            .or_else(|| by_extension.clone());
        let preview = sniffed
            .as_ref()
            .map(|s| s.preview)
            .or_else(|| by_extension.as_deref().map(mime::preview_for));

        let availability = if original.is_some() {
            PhotoAvailability::InBackup
        } else if row.is_cloud_only() {
            PhotoAvailability::CloudOnly
        } else {
            PhotoAvailability::Missing
        };

        let size = row
            .original_size
            .and_then(|s| u64::try_from(s).ok())
            .filter(|s| *s > 0)
            .or_else(|| {
                original
                    .as_ref()
                    .and_then(|e| e.meta.as_ref())
                    .map(|m| m.size)
            });

        let (edited_flag, live_flag, hidden_flag, location) = (
            row.is_edited(),
            row.is_live(),
            row.is_hidden(),
            row.location(),
        );

        PhotoAsset {
            id: row.pk.to_string(),
            uuid: row.uuid,
            kind: if row.kind == Some(KIND_VIDEO) {
                PhotoKind::Video
            } else {
                PhotoKind::Photo
            },
            file_id: original.as_ref().map(|e| e.file_id.clone()),
            thumb_file_id: thumb.as_ref().map(|e| e.file_id.clone()),
            live_file_id: live.as_ref().map(|e| e.file_id.clone()),
            edited_file_id: edited.as_ref().map(|e| e.file_id.clone()),
            edited_live_file_id: edited_live.as_ref().map(|e| e.file_id.clone()),
            original_file_name: row
                .original_file_name
                .filter(|name| name.trim() != row.file_name.trim())
                .filter(|name| !name.trim().is_empty()),
            file_name: row.file_name,
            mime,
            preview,
            created_at: row.date.and_then(from_apple),
            added_at: row.added.and_then(from_apple),
            modified_at: row.modified.and_then(from_apple),
            width: row.width.unwrap_or(0).max(0) as u32,
            height: row.height.unwrap_or(0).max(0) as u32,
            duration: row.duration.filter(|d| *d > 0.0),
            size,
            favorite: row.favorite.unwrap_or(0) != 0,
            screenshot: row.screenshot.unwrap_or(0) != 0,
            edited: edited_flag,
            live: live_flag,
            trashed: row.trashed.unwrap_or(0) != 0,
            hidden: hidden_flag,
            location,
            availability,
        }
    }

    /// Look up one path, requiring its bytes to be in the backup.
    fn resolve(&self, relative: &str) -> Option<ManifestEntry> {
        self.backup
            .find(DOMAIN, relative)
            .ok()
            .flatten()
            .filter(|e| self.backup.has_data(e))
    }

    /// The first of several candidate paths that resolves.
    fn resolve_any(&self, candidates: impl Iterator<Item = String>) -> Option<ManifestEntry> {
        candidates.filter_map(|path| self.resolve(&path)).next()
    }
}

/// One row of the asset query, before resolution.
struct AssetRow {
    pk: i64,
    uuid: Option<String>,
    directory: String,
    file_name: String,
    kind: Option<i64>,
    date: Option<f64>,
    added: Option<f64>,
    modified: Option<f64>,
    width: Option<i64>,
    height: Option<i64>,
    duration: Option<f64>,
    favorite: Option<i64>,
    screenshot: Option<i64>,
    adjustments: Option<i64>,
    kind_subtype: Option<i64>,
    playback_style: Option<i64>,
    trashed: Option<i64>,
    hidden: Option<i64>,
    visibility: Option<i64>,
    latitude: Option<f64>,
    longitude: Option<f64>,
    cloud_local_state: Option<i64>,
    complete: Option<i64>,
    original_file_name: Option<String>,
    original_size: Option<i64>,
}

impl AssetRow {
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            pk: row.get(0)?,
            uuid: row.get(1)?,
            directory: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            file_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            kind: row.get(4)?,
            date: row.get(5)?,
            added: row.get(6)?,
            modified: row.get(7)?,
            width: row.get(8)?,
            height: row.get(9)?,
            duration: row.get(10)?,
            favorite: row.get(11)?,
            screenshot: row.get(12)?,
            adjustments: row.get(13)?,
            kind_subtype: row.get(14)?,
            playback_style: row.get(15)?,
            trashed: row.get(16)?,
            hidden: row.get(17)?,
            visibility: row.get(18)?,
            latitude: row.get(19)?,
            longitude: row.get(20)?,
            cloud_local_state: row.get(21)?,
            complete: row.get(22)?,
            original_file_name: row.get(23)?,
            original_size: row.get(24)?,
        })
    }

    /// Either signal marks a Live Photo; `ZAVALANCHEUUID` does not.
    fn is_live(&self) -> bool {
        self.kind_subtype == Some(SUBTYPE_LIVE) || self.playback_style == Some(PLAYBACK_LIVE)
    }

    fn is_edited(&self) -> bool {
        self.adjustments.unwrap_or(0) != 0
    }

    fn is_hidden(&self) -> bool {
        self.hidden.unwrap_or(0) != 0 || self.visibility.unwrap_or(0) != 0
    }

    /// Apple marks an asset whose bytes were never downloaded with a non-zero
    /// `ZCLOUDLOCALSTATE`, or by leaving `ZCOMPLETE` unset.
    fn is_cloud_only(&self) -> bool {
        self.cloud_local_state.unwrap_or(0) != 0 || self.complete == Some(0)
    }

    fn location(&self) -> Option<GeoPoint> {
        let (Some(latitude), Some(longitude)) = (self.latitude, self.longitude) else {
            return None;
        };
        // -180 is Apple's "no location", not a real coordinate, and it was the
        // value for every asset in the real library.
        if latitude == NO_COORDINATE || longitude == NO_COORDINATE {
            return None;
        }
        if !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) {
            return None;
        }
        Some(GeoPoint {
            latitude,
            longitude,
        })
    }
}

/// Find the album-membership join table.
///
/// The documented approach — scan `sqlite_master` for `Z_%ASSETS` — is
/// ambiguous: the real library has **eleven** such tables for memories,
/// suggestions and key assets. Core Data's own entity map resolves it: the
/// `Album` entity's number names the table, so `Album` = 34 gives
/// `Z_34ASSETS` with columns `Z_34ALBUMS` and `Z_3ASSETS`.
fn discover_album_join(conn: &Connection) -> Option<AlbumJoin> {
    // The entity map names both sides: `Album` numbers the table and its album
    // column, `Asset` numbers the asset column.
    let entity = |name: &str| {
        conn.query_one(
            "SELECT Z_ENT FROM Z_PRIMARYKEY WHERE Z_NAME = ?1",
            [name],
            |row| row.get::<_, i64>(0),
        )
        .ok()
    };

    if let (Some(album_ent), Some(asset_ent)) = (entity("Album"), entity("Asset")) {
        let table = format!("Z_{album_ent}ASSETS");
        let album_column = format!("Z_{album_ent}ALBUMS");
        let asset_column = format!("Z_{asset_ent}ASSETS");
        let columns = ordered_columns(conn, &table);
        if columns.contains(&album_column) && columns.contains(&asset_column) {
            return Some(AlbumJoin {
                table,
                album_column,
                asset_column,
            });
        }
    }

    // Fallback for a schema with no `Z_PRIMARYKEY`: the one `Z_<n>ASSETS`
    // table that also has a `Z_<n>ALBUMS` column. The memories and suggestions
    // tables do not, which is what makes this unambiguous.
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name LIKE 'Z\\_%ASSETS' ESCAPE '\\' ORDER BY name",
        )
        .ok()?;
    let tables: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .ok()?
        .flatten()
        .collect();
    for table in tables {
        let columns = ordered_columns(conn, &table);
        let Some(album_column) = columns.iter().find(|c| c.ends_with("ALBUMS")) else {
            continue;
        };
        let Some(asset_column) = pick_asset_column(&columns, album_column) else {
            continue;
        };
        return Some(AlbumJoin {
            table,
            album_column: album_column.clone(),
            asset_column,
        });
    }
    None
}

/// The asset side of a join table.
///
/// Core Data also stores an **ordering** column alongside a many-to-many
/// relationship — `Z_FOK_3ASSETS` beside `Z_3ASSETS` — and joining on that
/// instead silently returns the wrong rows: it reported 3 albums where the
/// real library has 7 with members. So the `Z_FOK_` columns are excluded
/// explicitly rather than left to iteration order.
fn pick_asset_column(columns: &[String], album_column: &str) -> Option<String> {
    columns
        .iter()
        .find(|c| {
            c.ends_with("ASSETS")
                && c.as_str() != album_column
                && !c.starts_with("Z_FOK")
                && !c.contains("FOK_")
        })
        .cloned()
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_one(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|n| n > 0)
    .unwrap_or(false)
}

/// Column names in schema order, for code that picks one of several.
fn ordered_columns(conn: &Connection, table: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info(?1) ORDER BY cid") else {
        return Vec::new();
    };
    stmt.query_map([table], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn table_columns(conn: &Connection, table: &str) -> HashSet<String> {
    let Ok(mut stmt) = conn.prepare("SELECT name FROM pragma_table_info(?1)") else {
        return HashSet::new();
    };
    stmt.query_map([table], |row| row.get::<_, String>(0))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

/// Rewrite bare `?` and `?x` markers as `?n`, `?n+1`, ... so clauses can be
/// composed without hand-counting positions.
fn renumber(sql: &str, start: usize) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut n = start;
    let mut chars = sql.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '?' {
            out.push(ch);
            continue;
        }
        // Drop any placeholder label that follows the '?'.
        while chars.peek().is_some_and(|c| c.is_ascii_alphanumeric()) {
            chars.next();
        }
        out.push('?');
        out.push_str(&n.to_string());
        n += 1;
    }
    out
}

/// `IMG_0001.HEIC` -> `IMG_0001`.
fn file_stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(at) if at > 0 => &name[..at],
        _ => name,
    }
}

/// Swap a path's extension, keeping its directory and stem.
fn replace_extension(path: &str, extension: &str) -> String {
    let (dir, name) = match path.rfind('/') {
        Some(at) => (&path[..=at], &path[at + 1..]),
        None => ("", path),
    };
    format!("{dir}{}.{extension}", file_stem(name))
}

pub mod dcim;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{FixtureBackup, photos as fixture};
    use crate::{Backup, CacheDir};

    fn open_fixture(tmp: &std::path::Path) -> Backup {
        let root = FixtureBackup::new().with_photos().build(tmp).unwrap();
        Backup::open_with_cache(&root, None, CacheDir::at(tmp.join("cache"))).unwrap()
    }

    fn all(db: &PhotosDb<'_>) -> Vec<PhotoAsset> {
        db.list(&PhotoFilter::default(), PhotoCursor::first(500))
            .unwrap()
            .assets
    }

    fn by_id(assets: &[PhotoAsset], pk: i64) -> Option<&PhotoAsset> {
        assets.iter().find(|a| a.id == pk.to_string())
    }

    #[test]
    fn the_library_opens_and_reports_which_tables_it_found() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        assert_eq!(db.asset_table(), "ZASSET");
        // Discovered from Z_PRIMARYKEY, not hard-coded: the fixture uses a
        // different entity number from the real library's 34.
        assert_eq!(
            db.album_join_table(),
            Some(format!("Z_{}ASSETS", fixture::ALBUM_ENTITY).as_str())
        );
    }

    #[test]
    fn the_join_ignores_core_datas_ordering_column() {
        // `Z_FOK_3ASSETS` sits beside `Z_3ASSETS` and joining on it returns
        // the wrong rows. Against the real library that under-reported albums
        // with members as 3 when there are 7.
        let columns = vec![
            "Z_42ALBUMS".to_string(),
            "Z_3ASSETS".to_string(),
            "Z_FOK_3ASSETS".to_string(),
        ];
        assert_eq!(
            pick_asset_column(&columns, "Z_42ALBUMS"),
            Some("Z_3ASSETS".to_string())
        );
        // Even when the ordering column comes first.
        let reversed = vec![
            "Z_FOK_3ASSETS".to_string(),
            "Z_42ALBUMS".to_string(),
            "Z_3ASSETS".to_string(),
        ];
        assert_eq!(
            pick_asset_column(&reversed, "Z_42ALBUMS"),
            Some("Z_3ASSETS".to_string())
        );
    }

    #[test]
    fn detection_finds_the_library() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        assert!(Photos::detect(&backup).is_present());

        // A backup with no camera roll at all is Absent, not an error.
        let bare = FixtureBackup::new()
            .with_file("HomeDomain", "a.txt", b"x")
            .build(&tmp.path().join("bare"))
            .unwrap();
        let bare =
            Backup::open_with_cache(&bare, None, CacheDir::at(tmp.path().join("c2"))).unwrap();
        assert_eq!(Photos::detect(&bare), Availability::Absent);
    }

    #[test]
    fn the_default_grid_hides_trashed_and_hidden_assets() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        assert!(
            by_id(&assets, fixture::TRASHED_PK).is_none(),
            "a trashed asset must not appear in the main grid"
        );
        // The real library had ZHIDDEN = 0 on every row, so this asset is
        // hidden through ZVISIBILITYSTATE alone.
        assert!(
            by_id(&assets, fixture::HIDDEN_PK).is_none(),
            "ZVISIBILITYSTATE alone must be enough to hide an asset"
        );
        assert!(by_id(&assets, fixture::STILL_PK).is_some());
    }

    #[test]
    fn trashed_and_hidden_are_their_own_albums() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let trashed = db
            .list(
                &PhotoFilter {
                    trashed_only: true,
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, fixture::TRASHED_PK.to_string());
        assert!(trashed[0].trashed);

        let hidden = db
            .list(
                &PhotoFilter {
                    hidden_only: true,
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].id, fixture::HIDDEN_PK.to_string());
        assert!(hidden[0].hidden);
    }

    #[test]
    fn assets_resolve_to_their_files_and_to_apples_derivative() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let still = by_id(&assets, fixture::STILL_PK).unwrap();
        assert!(still.file_id.is_some(), "the original should resolve");
        assert!(
            still.thumb_file_id.is_some(),
            "the grid depends on Apple's derivative resolving"
        );
        assert_eq!(still.availability, PhotoAvailability::InBackup);

        // The Live Photo deliberately has no derivative, so the UI has to fall
        // back to the original.
        let live = by_id(&assets, fixture::LIVE_PK).unwrap();
        assert!(live.file_id.is_some());
        assert!(live.thumb_file_id.is_none());
    }

    #[test]
    fn a_cloud_only_asset_is_reported_rather_than_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let cloud = by_id(&assets, fixture::CLOUD_ONLY_PK)
            .expect("a cloud-only asset still belongs in the grid, as a placeholder");
        assert_eq!(cloud.availability, PhotoAvailability::CloudOnly);
        assert!(cloud.file_id.is_none());
    }

    #[test]
    fn a_live_photo_finds_its_companion_video() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let live = by_id(&assets, fixture::LIVE_PK).unwrap();
        assert!(live.live, "ZKINDSUBTYPE 2 marks a Live Photo");
        assert!(
            live.live_file_id.is_some(),
            "the .MOV beside the still is the companion"
        );
        // And the companion is not itself an asset: the fixture has ten rows.
        assert!(
            !assets
                .iter()
                .any(|a| a.file_name == fixture::LIVE_COMPANION),
            "the companion video must not be listed as its own asset"
        );
    }

    #[test]
    fn an_edited_asset_prefers_its_rendered_version() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let edited = by_id(&assets, fixture::EDITED_PK).unwrap();
        assert!(edited.edited);
        let render = edited
            .edited_file_id
            .as_deref()
            .expect("the FullSizeRender should resolve");
        assert_eq!(
            edited.display_file_id(),
            Some(render),
            "the lightbox shows the edit, as Photos does"
        );
        assert_ne!(
            edited.file_id.as_deref(),
            Some(render),
            "the original is a different file"
        );

        // An unedited asset displays its original.
        let still = by_id(&assets, fixture::STILL_PK).unwrap();
        assert_eq!(still.display_file_id(), still.file_id.as_deref());
    }

    #[test]
    fn the_media_type_comes_from_the_file_not_the_recorded_uti() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        // The library records `public.jpeg` for this one; the file is a PNG.
        // 578 real assets lie this way.
        let screenshot = by_id(&assets, fixture::SCREENSHOT_PK).unwrap();
        assert_eq!(screenshot.mime.as_deref(), Some("image/png"));
        assert!(screenshot.screenshot);
    }

    #[test]
    fn kind_video_and_duration_survive() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let video = by_id(&assets, fixture::VIDEO_PK).unwrap();
        assert_eq!(video.kind, PhotoKind::Video);
        assert_eq!(video.duration, Some(12.5));

        let videos = db
            .list(
                &PhotoFilter {
                    kind: Some(PhotoKind::Video),
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(videos.len(), 1);

        let photos = db
            .list(
                &PhotoFilter {
                    kind: Some(PhotoKind::Photo),
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert!(!photos.is_empty());
        assert!(photos.iter().all(|a| a.kind == PhotoKind::Photo));
    }

    #[test]
    fn favorites_and_screenshots_filter() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let favorites = db
            .list(
                &PhotoFilter {
                    favorites_only: true,
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(favorites.len(), 1);
        assert!(favorites[0].favorite);

        let screenshots = db
            .list(
                &PhotoFilter {
                    screenshots_only: true,
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(screenshots.len(), 1);
        assert!(screenshots[0].screenshot);
    }

    #[test]
    fn only_titled_albums_are_listed_with_their_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let albums = db.albums().unwrap();
        assert_eq!(albums.len(), 1, "an untitled album is noise in a picker");
        assert_eq!(albums[0].title, fixture::ALBUM_TITLE);
        assert_eq!(albums[0].count, 2);

        // And filtering by it returns exactly those assets.
        let in_album = db
            .list(
                &PhotoFilter {
                    album_id: Some(albums[0].id.clone()),
                    ..Default::default()
                },
                PhotoCursor::first(500),
            )
            .unwrap()
            .assets;
        assert_eq!(in_album.len(), 2);
        assert!(by_id(&in_album, fixture::ALBUM_PK).is_some());
        assert!(by_id(&in_album, fixture::VIDEO_PK).is_some());
    }

    #[test]
    fn the_grid_is_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        let dates: Vec<_> = assets.iter().filter_map(|a| a.created_at).collect();
        assert!(dates.len() > 1);
        assert!(
            dates.windows(2).all(|w| w[0] >= w[1]),
            "the grid reads newest first"
        );
    }

    #[test]
    fn keyset_paging_loses_nothing_and_repeats_nothing() {
        // The test that caught the WhatsApp paging bug, applied here.
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let one_shot = all(&db);
        let mut paged = Vec::new();
        let mut cursor = Some(PhotoCursor::first(2));
        while let Some(c) = cursor {
            let page = db.list(&PhotoFilter::default(), c).unwrap();
            paged.extend(page.assets);
            cursor = page.older;
        }

        assert_eq!(paged.len(), one_shot.len());
        let ids: std::collections::HashSet<&String> = paged.iter().map(|a| &a.id).collect();
        assert_eq!(ids.len(), paged.len(), "an asset was returned twice");
        assert_eq!(
            paged.iter().map(|a| &a.id).collect::<Vec<_>>(),
            one_shot.iter().map(|a| &a.id).collect::<Vec<_>>(),
            "paging must not reorder the grid"
        );
    }

    #[test]
    fn the_timeline_sums_to_the_grid() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let months = db.timeline(&PhotoFilter::default()).unwrap();
        assert!(months.len() >= 2, "the fixture spans two months");
        let total: u64 = months.iter().map(|m| m.count).sum();
        assert_eq!(total, db.count(&PhotoFilter::default()).unwrap());
        assert_eq!(total as usize, all(&db).len());

        // Newest month first, matching the grid.
        let keyed: Vec<(i32, u32)> = months.iter().map(|m| (m.year, m.month)).collect();
        let mut sorted = keyed.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(keyed, sorted);
    }

    #[test]
    fn a_single_asset_can_be_fetched_for_the_lightbox() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();

        let asset = db
            .asset(&fixture::ALBUM_PK.to_string())
            .unwrap()
            .expect("the asset should be found");
        assert_eq!(asset.id, fixture::ALBUM_PK.to_string());
        // The imported name differs from the name on disk, so both are kept.
        assert_eq!(asset.file_name, fixture::ALBUM_FILE);
        assert_eq!(
            asset.original_file_name.as_deref(),
            Some(fixture::ALBUM_ORIGINAL_NAME)
        );

        // A hidden or trashed asset is still fetchable by id: the lightbox is
        // reached from a filtered grid that already decided to show it.
        assert!(
            db.asset(&fixture::TRASHED_PK.to_string())
                .unwrap()
                .is_some()
        );
        // Unknown and non-numeric ids are None, not errors.
        assert!(db.asset("999999").unwrap().is_none());
        assert!(db.asset("not-a-number").unwrap().is_none());
    }

    #[test]
    fn location_uses_none_for_apples_minus_180_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let backup = open_fixture(tmp.path());
        let db = PhotosDb::open(&backup).unwrap();
        let assets = all(&db);

        // Every asset in the real library had -180, which is "no location".
        let still = by_id(&assets, fixture::STILL_PK).unwrap();
        assert_eq!(still.location, None);

        let located = by_id(&assets, fixture::ALBUM_PK).unwrap();
        let point = located.location.expect("real coordinates should survive");
        assert!((point.latitude - 51.5007).abs() < 1e-6);
        assert!((point.longitude - -0.1246).abs() < 1e-6);
    }

    #[test]
    fn a_cursor_from_ipc_is_clamped() {
        assert_eq!(PhotoCursor::first(0).limit, 1);
        assert_eq!(PhotoCursor::first(99_999).limit, PhotoCursor::MAX_LIMIT);
        let hostile = PhotoCursor {
            before_date: None,
            before_pk: None,
            limit: u32::MAX,
        };
        assert_eq!(hostile.clamped_limit(), PhotoCursor::MAX_LIMIT);
    }

    #[test]
    fn path_helpers_handle_names_without_extensions() {
        assert_eq!(file_stem("IMG_0001.HEIC"), "IMG_0001");
        assert_eq!(file_stem("IMG_0001"), "IMG_0001");
        // A leading dot is not an extension separator.
        assert_eq!(file_stem(".hidden"), ".hidden");
        assert_eq!(
            replace_extension("Media/DCIM/100APPLE/IMG_0001.HEIC", "MOV"),
            "Media/DCIM/100APPLE/IMG_0001.MOV"
        );
        assert_eq!(replace_extension("IMG_0001", "MOV"), "IMG_0001.MOV");
    }

    #[test]
    fn renumbering_rewrites_placeholders_in_order() {
        assert_eq!(renumber("a = ? AND b = ?", 1), "a = ?1 AND b = ?2");
        assert_eq!(renumber("a = ?a AND b = ?b", 3), "a = ?3 AND b = ?4");
        assert_eq!(renumber("no placeholders", 1), "no placeholders");
    }
}

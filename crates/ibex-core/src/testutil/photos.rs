//! A synthetic `Photos.sqlite`, plus the camera-roll files it points at.
//!
//! The schema mirrors a real iOS 27.2 library, and the contents cover the
//! cases real data proved easy to get wrong:
//!
//! - an asset whose `ZUNIFORMTYPEIDENTIFIER` contradicts its own file (578 of
//!   these in the real library), so the extractor must not trust that column;
//! - a hidden asset flagged only through `ZVISIBILITYSTATE`, because real
//!   `ZHIDDEN` was 0 for all 7,865 rows;
//! - a Live Photo, whose companion `.MOV` is a sibling file and not an asset
//!   row of its own;
//! - an edited asset with a `FullSizeRender` under `Mutations/`;
//! - a trashed asset, which the default grid must exclude;
//! - a cloud-only asset with no file in the backup at all;
//! - an album join table numbered **differently** from the real library's
//!   `Z_34ASSETS`, so the discovery code is exercised rather than a constant.
//!
//! Every file name and album title is invented (AGENTS.md hard rule 2).

use chrono::{TimeZone, Utc};

use crate::util::time::to_apple_secs;

/// `Z_PK` of a plain HEIC still.
pub const STILL_PK: i64 = 1;
/// A JPEG still that belongs to the album.
pub const ALBUM_PK: i64 = 2;
/// A video.
pub const VIDEO_PK: i64 = 3;
/// A Live Photo: a still with a `.MOV` beside it.
pub const LIVE_PK: i64 = 4;
/// An edited asset, with a rendered version under `Mutations/`.
pub const EDITED_PK: i64 = 5;
/// A screenshot, whose recorded UTI says JPEG while the file is a PNG.
pub const SCREENSHOT_PK: i64 = 6;
/// Hidden through `ZVISIBILITYSTATE` only, as real data does it.
pub const HIDDEN_PK: i64 = 7;
/// In the Recently Deleted album.
pub const TRASHED_PK: i64 = 8;
/// In the library but never downloaded: no file in the backup.
pub const CLOUD_ONLY_PK: i64 = 9;
/// A favourite.
pub const FAVORITE_PK: i64 = 10;

/// The album's primary key, and its title.
pub const ALBUM_ID: i64 = 71;
pub const ALBUM_TITLE: &str = "Fixture Album";
/// An album row with no title, which the picker must leave out.
pub const UNTITLED_ALBUM_ID: i64 = 72;

/// The `Album` entity number, deliberately not the real library's 34.
pub const ALBUM_ENTITY: i64 = 42;
/// The `Asset` entity number.
pub const ASSET_ENTITY: i64 = 3;

/// The roll directory every fixture asset lives in.
pub const DIRECTORY: &str = "DCIM/100APPLE";

/// File names, as `ZFILENAME` holds them.
pub const STILL_FILE: &str = "FIXTURE_0001.HEIC";
pub const ALBUM_FILE: &str = "FIXTURE_0002.JPG";
pub const VIDEO_FILE: &str = "FIXTURE_0003.MP4";
pub const LIVE_FILE: &str = "FIXTURE_0004.HEIC";
pub const LIVE_COMPANION: &str = "FIXTURE_0004.MOV";
pub const EDITED_FILE: &str = "FIXTURE_0005.JPG";
pub const SCREENSHOT_FILE: &str = "FIXTURE_0006.PNG";
pub const HIDDEN_FILE: &str = "FIXTURE_0007.JPG";
pub const TRASHED_FILE: &str = "FIXTURE_0008.JPG";
pub const CLOUD_ONLY_FILE: &str = "FIXTURE_0009.HEIC";
pub const FAVORITE_FILE: &str = "FIXTURE_0010.JPG";

/// The name `ZORIGINALFILENAME` carries for [`ALBUM_PK`], which differs from
/// the name on disk.
pub const ALBUM_ORIGINAL_NAME: &str = "imported-from-camera.jpg";

/// Where the file for an asset lives, relative to `CameraRollDomain`.
pub fn media_path(file_name: &str) -> String {
    format!("Media/{DIRECTORY}/{file_name}")
}

/// Where Apple's pre-rendered derivative lives for an asset.
pub fn thumb_path(file_name: &str) -> String {
    format!("Media/PhotoData/Thumbnails/V2/{DIRECTORY}/{file_name}/5005.JPG")
}

/// Where the rendered edit lives for an asset.
pub fn render_path(file_name: &str, extension: &str) -> String {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    format!("Media/PhotoData/Mutations/{DIRECTORY}/{stem}/Adjustments/FullSizeRender.{extension}")
}

/// A fixed date per asset, so ordering assertions are stable.
fn created(pk: i64) -> f64 {
    // 2026-03-01T12:00:00Z, one day apart, newest for the lowest primary key
    // so the grid's "newest first" is not accidentally the same as "by id".
    let base = Utc.with_ymd_and_hms(2026, 3, 1, 12, 0, 0).unwrap();
    let days = chrono::Duration::days(10 - pk);
    to_apple_secs(base + days)
}

/// An asset spread across two months, so the timeline has more than one row.
fn created_other_month() -> f64 {
    to_apple_secs(Utc.with_ymd_and_hms(2025, 12, 24, 9, 30, 0).unwrap())
}

/// One row of the synthetic library, before it is inserted.
type AssetRow<'a> = (
    i64,     // pk
    &'a str, // file
    i64,     // kind
    i64,     // subtype
    i64,     // playback
    i64,     // favorite
    i64,     // screenshot
    i64,     // adjustments
    i64,     // trashed
    i64,     // hidden
    i64,     // visibility
    &'a str, // uti
    i64,     // width
    i64,     // height
    f64,     // duration
);

/// `CREATE TABLE` plus `INSERT` statements for the synthetic library.
pub fn photos_sql() -> String {
    let mut sql = String::from(
        "CREATE TABLE Z_PRIMARYKEY (
             Z_ENT INTEGER PRIMARY KEY, Z_NAME VARCHAR, Z_SUPER INTEGER, Z_MAX INTEGER);

         CREATE TABLE ZASSET (
             Z_PK INTEGER PRIMARY KEY,
             Z_ENT INTEGER,
             ZUUID VARCHAR,
             ZDIRECTORY VARCHAR,
             ZFILENAME VARCHAR,
             ZKIND INTEGER,
             ZKINDSUBTYPE INTEGER,
             ZPLAYBACKSTYLE INTEGER,
             ZDATECREATED TIMESTAMP,
             ZADDEDDATE TIMESTAMP,
             ZMODIFICATIONDATE TIMESTAMP,
             ZWIDTH INTEGER,
             ZHEIGHT INTEGER,
             ZDURATION FLOAT,
             ZFAVORITE INTEGER,
             ZISDETECTEDSCREENSHOT INTEGER,
             ZADJUSTMENTSSTATE INTEGER,
             ZTRASHEDSTATE INTEGER,
             ZHIDDEN INTEGER,
             ZVISIBILITYSTATE INTEGER,
             ZLATITUDE FLOAT,
             ZLONGITUDE FLOAT,
             ZCLOUDLOCALSTATE INTEGER,
             ZCOMPLETE INTEGER,
             ZAVALANCHEUUID VARCHAR,
             ZUNIFORMTYPEIDENTIFIER VARCHAR,
             ZADDITIONALATTRIBUTES INTEGER);

         CREATE TABLE ZADDITIONALASSETATTRIBUTES (
             Z_PK INTEGER PRIMARY KEY,
             ZASSET INTEGER,
             ZORIGINALFILENAME VARCHAR,
             ZORIGINALFILESIZE INTEGER);

         CREATE TABLE ZGENERICALBUM (
             Z_PK INTEGER PRIMARY KEY, Z_ENT INTEGER, ZKIND INTEGER, ZTITLE VARCHAR);
        ",
    );

    // The join table, named from the Album entity number rather than hard-coded.
    sql.push_str(&format!(
        // `Z_FOK_...ASSETS` is Core Data's ordering column. It is here on
        // purpose: joining on it instead of the real foreign key silently
        // returns the wrong rows, which is exactly what happened against real
        // data (3 albums reported where 7 have members).
        "CREATE TABLE \"Z_{ALBUM_ENTITY}ASSETS\" (
             Z_{ALBUM_ENTITY}ALBUMS INTEGER,
             Z_{ASSET_ENTITY}ASSETS INTEGER,
             Z_FOK_{ASSET_ENTITY}ASSETS INTEGER,
             PRIMARY KEY (Z_{ALBUM_ENTITY}ALBUMS, Z_{ASSET_ENTITY}ASSETS));

         -- A memories join table, to prove the discovery code is not just
         -- picking the first `Z_%ASSETS` table it sees. Real libraries have
         -- eleven of these.
         CREATE TABLE Z_3MEMORIESBEINGCURATEDASSETS (
             Z_3CURATEDASSETS INTEGER,
             Z_59MEMORIESBEINGCURATEDASSETS INTEGER);
        "
    ));

    sql.push_str(&format!(
        "INSERT INTO Z_PRIMARYKEY (Z_ENT, Z_NAME, Z_SUPER, Z_MAX) VALUES
             ({ASSET_ENTITY}, 'Asset', 0, 10),
             (41, 'GenericAlbum', 0, 2),
             ({ALBUM_ENTITY}, 'Album', 41, 0),
             (59, 'Memory', 0, 0);
        "
    ));

    // (pk, file, kind, subtype, playback, favorite, screenshot, adjustments,
    //  trashed, hidden, visibility, uti, width, height, duration)
    let rows: &[AssetRow<'_>] = &[
        (
            STILL_PK,
            STILL_FILE,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            "public.heic",
            4032,
            3024,
            0.0,
        ),
        (
            ALBUM_PK,
            ALBUM_FILE,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            "public.jpeg",
            1600,
            1200,
            0.0,
        ),
        (
            VIDEO_PK,
            VIDEO_FILE,
            1,
            0,
            4,
            0,
            0,
            0,
            0,
            0,
            0,
            "public.mpeg-4",
            1920,
            1080,
            12.5,
        ),
        (
            LIVE_PK,
            LIVE_FILE,
            0,
            2,
            3,
            0,
            0,
            0,
            0,
            0,
            0,
            "public.heic",
            4032,
            3024,
            0.0,
        ),
        (
            EDITED_PK,
            EDITED_FILE,
            0,
            0,
            1,
            0,
            0,
            2,
            0,
            0,
            0,
            "public.jpeg",
            3024,
            4032,
            0.0,
        ),
        // The trap: recorded as JPEG, actually a PNG on disk.
        (
            SCREENSHOT_PK,
            SCREENSHOT_FILE,
            0,
            0,
            1,
            0,
            1,
            0,
            0,
            0,
            0,
            "public.jpeg",
            1179,
            2556,
            0.0,
        ),
        // Hidden through ZVISIBILITYSTATE alone, with ZHIDDEN left at 0.
        (
            HIDDEN_PK,
            HIDDEN_FILE,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            2,
            "public.jpeg",
            800,
            600,
            0.0,
        ),
        (
            TRASHED_PK,
            TRASHED_FILE,
            0,
            0,
            1,
            0,
            0,
            0,
            1,
            0,
            0,
            "public.jpeg",
            800,
            600,
            0.0,
        ),
        (
            CLOUD_ONLY_PK,
            CLOUD_ONLY_FILE,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            0,
            "public.heic",
            4032,
            3024,
            0.0,
        ),
        (
            FAVORITE_PK,
            FAVORITE_FILE,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
            0,
            0,
            "public.jpeg",
            900,
            900,
            0.0,
        ),
    ];

    for (
        pk,
        file,
        kind,
        subtype,
        playback,
        favorite,
        screenshot,
        adjustments,
        trashed,
        hidden,
        visibility,
        uti,
        width,
        height,
        duration,
    ) in rows
    {
        // One asset sits in a different month so the timeline has two rows.
        let date = if *pk == FAVORITE_PK {
            created_other_month()
        } else {
            created(*pk)
        };
        // Only the cloud-only asset is marked as not downloaded.
        let (cloud_state, complete) = if *pk == CLOUD_ONLY_PK { (1, 0) } else { (0, 1) };
        // One asset carries real coordinates; everything else uses Apple's
        // -180 "no location" sentinel, as the whole real library did.
        let (latitude, longitude) = if *pk == ALBUM_PK {
            (51.5007, -0.1246)
        } else {
            (-180.0, -180.0)
        };
        sql.push_str(&format!(
            "INSERT INTO ZASSET (Z_PK, Z_ENT, ZUUID, ZDIRECTORY, ZFILENAME, ZKIND, ZKINDSUBTYPE,
                 ZPLAYBACKSTYLE, ZDATECREATED, ZADDEDDATE, ZMODIFICATIONDATE, ZWIDTH, ZHEIGHT,
                 ZDURATION, ZFAVORITE, ZISDETECTEDSCREENSHOT, ZADJUSTMENTSSTATE, ZTRASHEDSTATE,
                 ZHIDDEN, ZVISIBILITYSTATE, ZLATITUDE, ZLONGITUDE, ZCLOUDLOCALSTATE, ZCOMPLETE,
                 ZAVALANCHEUUID, ZUNIFORMTYPEIDENTIFIER, ZADDITIONALATTRIBUTES)
             VALUES ({pk}, {ASSET_ENTITY}, 'FIXTURE-UUID-{pk:04}', '{DIRECTORY}', '{file}',
                 {kind}, {subtype}, {playback}, {date}, {date}, {date}, {width}, {height},
                 {duration}, {favorite}, {screenshot}, {adjustments}, {trashed}, {hidden},
                 {visibility}, {latitude}, {longitude}, {cloud_state}, {complete}, NULL,
                 '{uti}', {pk});
             INSERT INTO ZADDITIONALASSETATTRIBUTES (Z_PK, ZASSET, ZORIGINALFILENAME,
                 ZORIGINALFILESIZE)
             VALUES ({pk}, {pk}, '{original}', 0);
            ",
            original = if *pk == ALBUM_PK {
                ALBUM_ORIGINAL_NAME
            } else {
                file
            },
        ));
    }

    sql.push_str(&format!(
        "INSERT INTO ZGENERICALBUM (Z_PK, Z_ENT, ZKIND, ZTITLE) VALUES
             ({ALBUM_ID}, {ALBUM_ENTITY}, 2, '{ALBUM_TITLE}'),
             ({UNTITLED_ALBUM_ID}, {ALBUM_ENTITY}, 2, NULL);
         INSERT INTO \"Z_{ALBUM_ENTITY}ASSETS\"
             (Z_{ALBUM_ENTITY}ALBUMS, Z_{ASSET_ENTITY}ASSETS, Z_FOK_{ASSET_ENTITY}ASSETS)
             VALUES ({ALBUM_ID}, {ALBUM_PK}, 0), ({ALBUM_ID}, {VIDEO_PK}, 1);
         INSERT INTO Z_3MEMORIESBEINGCURATEDASSETS VALUES ({STILL_PK}, 1);
        "
    ));

    sql
}

/// A minimal MP4 header, enough for the type sniffer.
pub fn mp4() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0, 0, 0, 0x18]);
    bytes.extend_from_slice(b"ftypmp42");
    bytes.extend_from_slice(b"\0\0\0\0mp42isom");
    bytes.extend_from_slice(&[0, 0, 0, 8]);
    bytes.extend_from_slice(b"free");
    bytes
}

/// A minimal QuickTime header, for Live Photo companions.
pub fn mov() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&[0, 0, 0, 0x14]);
    bytes.extend_from_slice(b"ftypqt  ");
    bytes.extend_from_slice(b"\0\0\x02\0qt  ");
    bytes.extend_from_slice(&[0, 0, 0, 8]);
    bytes.extend_from_slice(b"free");
    bytes
}

/// A small baseline JPEG, for the pre-rendered derivatives.
///
/// Encoded here rather than embedded as a blob so it is readable and so the
/// dimensions can be asserted.
pub fn jpeg(width: u32, height: u32) -> Vec<u8> {
    use image::{ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder};
    let mut pixels = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height {
        for x in 0..width {
            // A gradient, so a re-encode or a resize is visibly wrong if the
            // row stride is mishandled.
            pixels.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 128]);
        }
    }
    let mut out = Vec::new();
    JpegEncoder::new_with_quality(&mut out, 90)
        .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
        .expect("encoding a fixture JPEG cannot fail");
    out
}

/// An AVIF image, for exercising the libheif decode path.
///
/// Returns `None` when the system libheif has no AV1 encoder plugin, which is
/// why every caller treats this as optional.
///
/// **Why AVIF and not HEIC:** AVIF and HEIC are the same container with a
/// different codec inside, and libheif decodes both through the identical API,
/// planes and colour space this crate uses. HEIC *encoding* needs an HEVC
/// encoder plugin that is not installed on most Linux machines, while AV1
/// encoding usually is. So the fixture proves the pipeline and the real
/// backup's 4,211 HEICs prove the codec.
pub fn avif(width: u32, height: u32) -> Option<Vec<u8>> {
    use libheif_rs::{
        Channel, ColorSpace, CompressionFormat, EncoderQuality, HeifContext, Image, LibHeif,
        RgbChroma,
    };

    let lib = LibHeif::new();
    let mut encoder = lib.encoder_for_format(CompressionFormat::Av1).ok()?;
    encoder.set_quality(EncoderQuality::Lossy(80)).ok()?;

    let mut image = Image::new(width, height, ColorSpace::Rgb(RgbChroma::Rgb)).ok()?;
    image
        .create_plane(Channel::Interleaved, width, height, 8)
        .ok()?;
    {
        let planes = image.planes_mut();
        let plane = planes.interleaved?;
        let stride = plane.stride;
        for y in 0..height as usize {
            for x in 0..width as usize {
                let at = y * stride + x * 3;
                let Some(pixel) = plane.data.get_mut(at..at + 3) else {
                    continue;
                };
                pixel.copy_from_slice(&[(x % 256) as u8, (y % 256) as u8, 200]);
            }
        }
    }

    let mut ctx = HeifContext::new().ok()?;
    ctx.encode_image(&image, &mut encoder, None).ok()?;
    ctx.write_to_bytes().ok()
}

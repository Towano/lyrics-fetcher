use crate::model::Query;
use anyhow::{bail, ensure, Context, Result};
use lofty::config::{apply_global_options, GlobalOptions, ParseOptions, ParsingMode};
use lofty::file::{AudioFile, FileType, TaggedFileExt};
use lofty::probe::Probe;
use lofty::tag::ItemKey;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

const TAG_ALLOCATION_LIMIT: usize = 1024 * 1024;

/// Read identity and duration without modifying the audio file or decoding artwork.
/// An untagged filename is a search hint, never trusted matching evidence.
pub fn read_file(path: &Path) -> Result<Query> {
    ensure!(
        path.is_file(),
        "输入不是可读取的普通音频文件: {}",
        path.display()
    );
    let path = path.to_owned();
    // Lofty's allocation options are thread-local with no public getter. Isolate
    // them so a library caller's metadata settings are not overwritten.
    std::thread::Builder::new()
        .name("lyrics-metadata".into())
        .spawn(move || read_inner(&path))
        .context("无法启动音频元数据读取线程")?
        .join()
        .map_err(|_| anyhow::anyhow!("音频元数据读取失败"))?
}

fn read_inner(path: &Path) -> Result<Query> {
    let mut file =
        File::open(path).with_context(|| format!("无法打开音频文件 {}", path.display()))?;
    let mut magic = [0; 8];
    let count = file.read(&mut magic)?;
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("ncm"))
        || (count == 8 && &magic == b"CTENFDAM")
    {
        bail!("不支持加密 NCM 文件；请先转换为普通音频，或使用歌曲 ID / --input-type query");
    }
    file.seek(SeekFrom::Start(0))?;
    apply_global_options(
        GlobalOptions::new()
            .allocation_limit(TAG_ALLOCATION_LIMIT)
            .preserve_format_specific_items(false)
            .use_custom_resolvers(false),
    );
    read_audio(file, path)
}

fn read_audio<R: Read + Seek>(reader: R, path: &Path) -> Result<Query> {
    // Probe unbuffered: a BufReader's read-ahead could read artwork even when
    // the MP4 parser only asks for an atom header immediately preceding it.
    let probe = Probe::new(reader)
        .guess_file_type()
        .context("无法识别音频格式")?;
    let file_type = probe.file_type();
    let mut reader = probe.into_inner();
    let mut options = ParseOptions::new()
        .read_properties(true)
        .read_cover_art(false);
    let (tagged, mp4) = if file_type == Some(FileType::Mp4) {
        let metadata = Mp4Metadata::read(&mut reader)?;
        reader.seek(SeekFrom::Start(0))?;
        let properties_reader = Mp4PropertiesReader {
            reader: &mut reader,
            skipped: &metadata.skipped,
        };
        let tagged = Probe::with_file_type(properties_reader, FileType::Mp4)
            .options(options.read_tags(false).parsing_mode(ParsingMode::Strict))
            .read()
            .context("无法读取 MP4 音频属性")?;
        (tagged, Some(metadata))
    } else {
        let probe = Probe::new(BufReader::new(reader));
        let probe = match file_type {
            Some(file_type) => probe.set_file_type(file_type),
            None => probe,
        };
        let tagged = probe
            .options(options.read_tags(true))
            .read()
            .with_context(|| format!("无法读取音频标签 {}", path.display()))?;
        (tagged, None)
    };
    let primary = tagged.primary_tag();
    let mut tags = Vec::new();
    if let Some(primary) = primary {
        tags.push(primary);
    }
    tags.extend(
        tagged
            .tags()
            .iter()
            .filter(|tag| Some(tag.tag_type()) != primary.map(|tag| tag.tag_type())),
    );
    let first_text = |key| {
        tags.iter()
            .flat_map(|tag| tag.get_strings(key))
            .map(str::trim)
            .find(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let title = first_text(ItemKey::TrackTitle);
    let album = first_text(ItemKey::AlbumTitle);
    // Prefer a single tag's complete credits rather than combining stale ID3v1
    // and current ID3v2 artist lists. Lofty exposes multi-value text as items.
    let artists = tags
        .iter()
        .find_map(|tag| {
            let artists: Vec<_> = tag
                .get_strings(ItemKey::TrackArtist)
                .map(str::trim)
                .filter(|artist| !artist.is_empty())
                .map(str::to_owned)
                .collect();
            (!artists.is_empty()).then_some(artists)
        })
        .unwrap_or_default();
    let (title, artists, album) = match mp4 {
        Some(metadata) => (metadata.title, metadata.artists, metadata.album),
        None => (title, artists, album),
    };
    ensure!(artists.len() <= 64, "音频歌手标签过多");
    for field in title.iter().chain(album.iter()).chain(artists.iter()) {
        ensure!(field.len() <= 1024, "音频元数据字段超过 1024 bytes");
    }
    let duration = tagged.properties().duration().as_secs_f64();
    let duration = (duration.is_finite() && duration > 0.0).then_some(duration);
    let keywords = if title.is_some() {
        String::new()
    } else {
        path.file_stem()
            .map(|stem| stem.to_string_lossy().trim().to_owned())
            .unwrap_or_default()
    };
    Ok(Query {
        keywords,
        title,
        artists,
        album,
        duration,
    })
}

// Lofty 0.24 reads the complete ilst before checking read_cover_art(false).
// Read only the three iTunes identity fields; all other payloads are seek-only.
// Bounds/count limits apply to visited atoms, not opaque artwork/media payloads.
const MP4_ATOM_LIMIT: usize = 8192;
const MP4_DEPTH_LIMIT: usize = 8;
const MP4_FIELD_LIMIT: usize = 1024;

#[derive(Clone, Copy)]
enum Mp4Level {
    Root,
    Moov,
    Trak,
    Mdia,
    Minf,
    Stbl,
    Udta,
    Meta,
    Ilst,
    Text([u8; 4]),
}

#[derive(Default)]
struct Mp4Metadata {
    title: Option<String>,
    artists: Vec<String>,
    album: Option<String>,
    skipped: Vec<std::ops::Range<u64>>,
    atoms: usize,
    text_bytes: u64,
}

impl Mp4Metadata {
    fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        let end = reader.seek(SeekFrom::End(0))?;
        let mut metadata = Self::default();
        metadata.walk(reader, 0, end, Mp4Level::Root, 0)?;
        Ok(metadata)
    }

    fn walk<R: Read + Seek>(
        &mut self,
        reader: &mut R,
        mut position: u64,
        end: u64,
        level: Mp4Level,
        depth: usize,
    ) -> Result<()> {
        ensure!(depth <= MP4_DEPTH_LIMIT, "MP4 原子嵌套过深");
        while position < end {
            ensure!(end - position >= 8, "MP4 原子头不完整");
            self.atoms += 1;
            ensure!(self.atoms <= MP4_ATOM_LIMIT, "MP4 原子数量过多");
            reader.seek(SeekFrom::Start(position))?;
            let mut header = [0; 8];
            reader.read_exact(&mut header)?;
            let raw_size = u32::from_be_bytes(header[..4].try_into().unwrap());
            let kind: [u8; 4] = header[4..].try_into().unwrap();
            let (size, header_size) = match raw_size {
                0 => (end - position, 8),
                1 => {
                    ensure!(end - position >= 16, "MP4 扩展原子头不完整");
                    let mut extended = [0; 8];
                    reader.read_exact(&mut extended)?;
                    (u64::from_be_bytes(extended), 16)
                }
                size => (u64::from(size), 8),
            };
            ensure!(size >= header_size, "MP4 原子大小小于原子头");
            let atom_end = position.checked_add(size).context("MP4 原子大小溢出")?;
            ensure!(atom_end <= end, "MP4 原子超出父原子或文件边界");
            let content = position + header_size;
            // Lofty 0.24's property traversal assumes 8-byte structural headers
            // and mishandles size=0. Do not silently return incomplete properties.
            if matches!(
                level,
                Mp4Level::Root
                    | Mp4Level::Moov
                    | Mp4Level::Trak
                    | Mp4Level::Mdia
                    | Mp4Level::Minf
                    | Mp4Level::Stbl
            ) {
                ensure!(raw_size > 1, "暂不支持 MP4 属性区域的扩展或零大小原子");
            }
            if matches!(level, Mp4Level::Mdia) && &kind == b"mdhd" {
                ensure!(atom_end - content >= 4, "MP4 mdhd FullBox 头不完整");
                let mut full_box = [0; 4];
                reader.read_exact(&mut full_box)?;
                let minimum = match full_box[0] {
                    0 => 24,
                    1 => 36,
                    _ => bail!("不支持的 MP4 mdhd 版本"),
                };
                ensure!(atom_end - content >= minimum, "MP4 mdhd 属性不完整");
            }
            if matches!(level, Mp4Level::Stbl) && &kind == b"stts" {
                ensure!(atom_end - content >= 8, "MP4 stts 表头不完整");
                let mut header = [0; 8];
                reader.read_exact(&mut header)?;
                ensure!(header[..4] == [0; 4], "不支持的 MP4 stts 版本或标记");
                let count = u64::from(u32::from_be_bytes(header[4..].try_into().unwrap()));
                ensure!(
                    count <= (atom_end - content - 8) / 8,
                    "MP4 stts 项目超出原子边界"
                );
            }
            if matches!(level, Mp4Level::Moov) && matches!(&kind, b"udta" | b"meta") {
                self.skipped.push(content..atom_end);
            }
            let child = match (level, &kind) {
                (Mp4Level::Root, b"moov") => Some(Mp4Level::Moov),
                (Mp4Level::Moov, b"trak") => Some(Mp4Level::Trak),
                (Mp4Level::Trak, b"mdia") => Some(Mp4Level::Mdia),
                (Mp4Level::Mdia, b"minf") => Some(Mp4Level::Minf),
                (Mp4Level::Minf, b"stbl") => Some(Mp4Level::Stbl),
                (Mp4Level::Moov, b"udta") => Some(Mp4Level::Udta),
                (Mp4Level::Moov | Mp4Level::Udta, b"meta") => Some(Mp4Level::Meta),
                (Mp4Level::Meta, b"ilst") => Some(Mp4Level::Ilst),
                (Mp4Level::Ilst, b"\xa9nam" | b"\xa9ART" | b"\xa9alb") => {
                    Some(Mp4Level::Text(kind))
                }
                _ => None,
            };
            if let Some(child) = child {
                let child_start = if matches!(child, Mp4Level::Meta) {
                    ensure!(atom_end - content >= 4, "MP4 meta FullBox 头不完整");
                    let mut full_box = [0; 4];
                    reader.read_exact(&mut full_box)?;
                    ensure!(
                        full_box == [0; 4],
                        "不支持的 MP4 meta 版本或非 FullBox 布局"
                    );
                    content + 4
                } else {
                    content
                };
                self.walk(reader, child_start, atom_end, child, depth + 1)?;
            } else if let (Mp4Level::Text(field), b"data") = (level, &kind) {
                self.read_text(reader, content, atom_end, field)?;
            }
            // In particular, never inspect even the data header inside covr.
            reader.seek(SeekFrom::Start(atom_end))?;
            position = atom_end;
        }
        Ok(())
    }

    fn read_text<R: Read>(
        &mut self,
        reader: &mut R,
        start: u64,
        end: u64,
        field: [u8; 4],
    ) -> Result<()> {
        ensure!(end - start >= 8, "MP4 文本 data 头不完整");
        let length = end - start - 8;
        // UTF-16BE can use two input bytes per ASCII character. Check before
        // allocation/read as well as after decoding (UTF-8 may expand).
        ensure!(length <= (MP4_FIELD_LIMIT * 2) as u64, "MP4 文本字段过大");
        self.text_bytes += length;
        ensure!(
            self.text_bytes <= TAG_ALLOCATION_LIMIT as u64,
            "MP4 文本标签总大小超过限制"
        );
        let mut header = [0; 8];
        reader.read_exact(&mut header)?;
        let encoding = u32::from_be_bytes(header[..4].try_into().unwrap());
        ensure!(
            matches!(encoding, 1 | 2),
            "不支持的 MP4 文本编码: {encoding}"
        );
        if encoding == 1 {
            ensure!(
                length <= MP4_FIELD_LIMIT as u64,
                "音频元数据字段超过 1024 bytes"
            );
        } else {
            ensure!(length % 2 == 0, "MP4 UTF-16BE 文本长度不是偶数");
        }
        let mut bytes = vec![0; length as usize];
        reader.read_exact(&mut bytes)?;
        let text = if encoding == 1 {
            String::from_utf8(bytes).context("无效 MP4 UTF-8 文本")?
        } else {
            let words: Vec<_> = bytes
                .chunks_exact(2)
                .map(|word| u16::from_be_bytes([word[0], word[1]]))
                .collect();
            String::from_utf16(&words).context("无效 MP4 UTF-16BE 文本")?
        };
        let text = text
            .trim_start_matches('\u{feff}')
            .trim_end_matches('\0')
            .trim();
        ensure!(
            text.len() <= MP4_FIELD_LIMIT,
            "音频元数据字段超过 1024 bytes"
        );
        if text.is_empty() {
            return Ok(());
        }
        match &field {
            b"\xa9nam" if self.title.is_none() => self.title = Some(text.to_owned()),
            b"\xa9alb" if self.album.is_none() => self.album = Some(text.to_owned()),
            b"\xa9ART" => {
                ensure!(self.artists.len() < 64, "音频歌手标签过多");
                self.artists.push(text.to_owned());
            }
            _ => {}
        }
        Ok(())
    }
}

// Lofty's property parser never needs udta/meta payloads. Reject a malformed
// property's over-read into them rather than letting it touch artwork. Seeks
// remain unchanged, and no bytes/offsets are replaced or written.
struct Mp4PropertiesReader<'a, R> {
    reader: &'a mut R,
    skipped: &'a [std::ops::Range<u64>],
}

impl<R: Read + Seek> Read for Mp4PropertiesReader<'_, R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let position = self.reader.stream_position()?;
        let mut length = buffer.len();
        for range in self.skipped {
            if range.contains(&position) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "MP4 音频属性读取越界进入元数据区域",
                ));
            }
            if range.start > position {
                length = length.min(usize::try_from(range.start - position).unwrap_or(usize::MAX));
            }
        }
        self.reader.read(&mut buffer[..length])
    }
}

impl<R: Seek> Seek for Mp4PropertiesReader<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.reader.seek(position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn chunk(id: &[u8; 4], content: &[u8], destination: &mut Vec<u8>) {
        destination.extend_from_slice(id);
        destination.extend_from_slice(&(content.len() as u32).to_le_bytes());
        destination.extend_from_slice(content);
        if content.len() % 2 != 0 {
            destination.push(0);
        }
    }
    fn synchsafe(value: usize) -> [u8; 4] {
        [
            (value >> 21 & 127) as u8,
            (value >> 14 & 127) as u8,
            (value >> 7 & 127) as u8,
            (value & 127) as u8,
        ]
    }
    fn id3() -> Vec<u8> {
        let mut frames = Vec::new();
        for (key, value) in [
            (b"TIT2", "A Song (Live)"),
            (b"TPE1", "Alice\0Bob"),
            (b"TALB", "Live Album"),
        ] {
            let mut content = vec![3]; // ID3v2.4 UTF-8 text.
            content.extend_from_slice(value.as_bytes());
            frames.extend_from_slice(key);
            frames.extend_from_slice(&synchsafe(content.len()));
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(&content);
        }
        let mut tag = b"ID3\x04\x00\x00".to_vec();
        tag.extend_from_slice(&synchsafe(frames.len()));
        tag.extend_from_slice(&frames);
        tag
    }
    fn wav(tag: Option<&[u8]>) -> Vec<u8> {
        let mut body = b"WAVE".to_vec();
        let mut format = Vec::new();
        format.extend_from_slice(&1_u16.to_le_bytes()); // PCM.
        format.extend_from_slice(&1_u16.to_le_bytes()); // One channel.
        format.extend_from_slice(&8000_u32.to_le_bytes());
        format.extend_from_slice(&8000_u32.to_le_bytes());
        format.extend_from_slice(&1_u16.to_le_bytes());
        format.extend_from_slice(&8_u16.to_le_bytes());
        chunk(b"fmt ", &format, &mut body);
        if let Some(tag) = tag {
            chunk(b"ID3 ", tag, &mut body);
        }
        chunk(b"data", &vec![128; 8000], &mut body); // One second.
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&(body.len() as u32).to_le_bytes());
        file.extend_from_slice(&body);
        file
    }

    #[test]
    fn reads_generated_wav_with_id3_and_all_artists_without_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("wrong-name.wav");
        let fixture = wav(Some(&id3()));
        fs::write(&path, &fixture).unwrap();
        let query = read_file(&path).unwrap();
        assert_eq!(query.title.as_deref(), Some("A Song (Live)"));
        assert_eq!(query.artists, ["Alice", "Bob"]);
        assert_eq!(query.album.as_deref(), Some("Live Album"));
        assert_eq!(query.duration, Some(1.0));
        assert!(query.keywords.is_empty());
        assert_eq!(fs::read(&path).unwrap(), fixture);
    }

    #[test]
    fn filename_is_not_trusted_title_evidence() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Alice - A Song.wav");
        fs::write(&path, wav(None)).unwrap();
        let query = read_file(&path).unwrap();
        assert_eq!(query.keywords, "Alice - A Song");
        assert!(query.title.is_none());
        assert!(query.artists.is_empty());
        assert_eq!(query.duration, Some(1.0));
    }

    #[test]
    fn rejects_encrypted_ncm_and_malformed_audio() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["track.NCM", "disguised.wav"] {
            let path = directory.path().join(name);
            fs::write(&path, b"CTENFDAMfixture").unwrap();
            assert!(read_file(&path).unwrap_err().to_string().contains("NCM"));
        }
        let path = directory.path().join("broken.wav");
        fs::write(&path, b"not audio").unwrap();
        assert!(read_file(&path).is_err());
        assert!(read_file(directory.path()).is_err());
    }

    fn mp4_atom(kind: &[u8; 4], content: &[u8]) -> Vec<u8> {
        let mut atom = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        atom.extend_from_slice(kind);
        atom.extend_from_slice(content);
        atom
    }

    fn mp4_data(encoding: u32, bytes: &[u8]) -> Vec<u8> {
        let mut content = encoding.to_be_bytes().to_vec();
        content.extend_from_slice(&[0; 4]); // Locale.
        content.extend_from_slice(bytes);
        mp4_atom(b"data", &content)
    }

    fn mp4_identity() -> Vec<u8> {
        let title = mp4_atom(b"\xa9nam", &mp4_data(1, b"A Song (Live)"));
        let mut artists = mp4_data(1, b"Alice");
        let bob: Vec<_> = "Bob".encode_utf16().flat_map(u16::to_be_bytes).collect();
        artists.extend(mp4_data(2, &bob));
        let artists = mp4_atom(b"\xa9ART", &artists);
        let album = mp4_atom(b"\xa9alb", &mp4_data(1, b"Live Album"));
        [title, artists, album].concat()
    }

    fn mp4_artwork() -> Vec<u8> {
        // A generated, valid uncompressed 768x512 24-bit BMP (>1 MiB).
        let pixel_bytes = 768 * 512 * 3_u32;
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&(54 + pixel_bytes).to_le_bytes());
        bmp.extend_from_slice(&[0; 4]);
        bmp.extend_from_slice(&54_u32.to_le_bytes());
        bmp.extend_from_slice(&40_u32.to_le_bytes());
        bmp.extend_from_slice(&768_i32.to_le_bytes());
        bmp.extend_from_slice(&512_i32.to_le_bytes());
        bmp.extend_from_slice(&1_u16.to_le_bytes());
        bmp.extend_from_slice(&24_u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 4]); // BI_RGB.
        bmp.extend_from_slice(&pixel_bytes.to_le_bytes());
        bmp.extend_from_slice(&[0; 16]);
        bmp.resize(54 + pixel_bytes as usize, 77);
        mp4_atom(b"covr", &mp4_data(27, &bmp))
    }

    fn mp4_fixture(ilst: &[u8]) -> Vec<u8> {
        // Generated ISO BMFF audio file with eight silent AAC-LC frames at
        // 8000 Hz. Each raw AAC block is SCE(tag=0, global_gain=100,
        // ONLY_LONG_SEQUENCE, max_sfb=0, no pulse/TNS/gain data), then END.
        // All spectral coefficients are zero; no third-party audio is embedded.
        let ftyp = mp4_atom(b"ftyp", b"M4A \0\0\0\0M4A isommp42");
        let mdat = mp4_atom(b"mdat", &[0x00, 0xc8, 0x00, 0x07].repeat(8));
        let mut mvhd = vec![0; 100];
        mvhd[12..16].copy_from_slice(&8000_u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&8192_u32.to_be_bytes());
        mvhd[20..24].copy_from_slice(&0x00010000_u32.to_be_bytes()); // Rate.
        mvhd[24..26].copy_from_slice(&0x0100_u16.to_be_bytes()); // Volume.
        for (offset, value) in [(36, 0x10000_u32), (52, 0x10000), (68, 0x40000000)] {
            mvhd[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        mvhd[96..100].copy_from_slice(&2_u32.to_be_bytes()); // Next track ID.
        let mut tkhd = vec![0; 84];
        tkhd[3] = 3; // Enabled, in movie.
        tkhd[12..16].copy_from_slice(&1_u32.to_be_bytes());
        tkhd[20..24].copy_from_slice(&8192_u32.to_be_bytes());
        tkhd[36..38].copy_from_slice(&0x0100_u16.to_be_bytes());
        for (offset, value) in [(40, 0x10000_u32), (56, 0x10000), (72, 0x40000000)] {
            tkhd[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        }
        let mut mdhd = vec![0; 24];
        mdhd[12..16].copy_from_slice(&8000_u32.to_be_bytes());
        mdhd[16..20].copy_from_slice(&8192_u32.to_be_bytes());
        mdhd[20..22].copy_from_slice(&0x55c4_u16.to_be_bytes()); // und.
        let mut hdlr = vec![0; 8];
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0; 12]);
        hdlr.extend_from_slice(b"Sound\0");
        let mut sample = vec![0; 28];
        sample[6..8].copy_from_slice(&1_u16.to_be_bytes()); // Data reference.
        sample[16..18].copy_from_slice(&1_u16.to_be_bytes()); // Mono.
        sample[18..20].copy_from_slice(&16_u16.to_be_bytes());
        sample[24..28].copy_from_slice(&(8000_u32 << 16).to_be_bytes());
        let mut esds = vec![0; 4];
        esds.extend_from_slice(&[3, 25, 0, 1, 0, 4, 17, 0x40, 0x15, 0, 0, 0]);
        esds.extend_from_slice(&[0; 8]); // Optional bitrates unspecified.
        esds.extend_from_slice(&[5, 2, 0x15, 0x88, 6, 1, 2]); // AAC-LC 8kHz mono.
        sample.extend(mp4_atom(b"esds", &esds));
        let mut stsd = vec![0; 4];
        stsd.extend_from_slice(&1_u32.to_be_bytes());
        stsd.extend(mp4_atom(b"mp4a", &sample));
        let table = |kind, entries: &[u32]| {
            let mut content = vec![0; 4];
            for entry in entries {
                content.extend_from_slice(&entry.to_be_bytes());
            }
            mp4_atom(kind, &content)
        };
        let stbl = mp4_atom(
            b"stbl",
            &[
                mp4_atom(b"stsd", &stsd),
                table(b"stts", &[1, 8, 1024]),
                table(b"stsc", &[1, 1, 8, 1]),
                table(b"stsz", &[4, 8]),
                table(b"stco", &[1, ftyp.len() as u32 + 8]),
            ]
            .concat(),
        );
        let mut dref = vec![0; 4];
        dref.extend_from_slice(&1_u32.to_be_bytes());
        dref.extend(mp4_atom(b"url ", &[0, 0, 0, 1]));
        let minf = mp4_atom(
            b"minf",
            &[
                mp4_atom(b"smhd", &[0; 8]),
                mp4_atom(b"dinf", &mp4_atom(b"dref", &dref)),
                stbl,
            ]
            .concat(),
        );
        let mdia = mp4_atom(
            b"mdia",
            &[mp4_atom(b"mdhd", &mdhd), mp4_atom(b"hdlr", &hdlr), minf].concat(),
        );
        let trak = mp4_atom(b"trak", &[mp4_atom(b"tkhd", &tkhd), mdia].concat());
        let meta = mp4_atom(b"meta", &[vec![0; 4], ilst.to_vec()].concat());
        let moov = mp4_atom(
            b"moov",
            &[mp4_atom(b"mvhd", &mvhd), trak, mp4_atom(b"udta", &meta)].concat(),
        );
        [ftyp, mdat, moov].concat()
    }

    struct CountingReader<'a> {
        reader: std::io::Cursor<&'a [u8]>,
        forbidden: std::ops::Range<u64>,
        bytes_read: usize,
        seeks: usize,
    }

    impl Read for CountingReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let start = self.reader.position();
            let end = start
                + (buffer.len() as u64)
                    .min((self.reader.get_ref().len() as u64).saturating_sub(start));
            assert!(
                start == end || end <= self.forbidden.start || start >= self.forbidden.end,
                "read would touch artwork: {start}..{end}"
            );
            let read = self.reader.read(buffer)?;
            self.bytes_read += read;
            Ok(read)
        }
    }

    impl Seek for CountingReader<'_> {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.seeks += 1;
            self.reader.seek(position)
        }
    }

    #[test]
    fn reads_generated_m4a_with_large_artwork_without_reading_or_writing_it() {
        let artwork = mp4_artwork();
        assert!(artwork.len() > TAG_ALLOCATION_LIMIT);
        // Put cover first so reading identity requires seeking across it.
        let ilst = mp4_atom(b"ilst", &[artwork.clone(), mp4_identity()].concat());
        let fixture = mp4_fixture(&ilst);
        let covr = fixture
            .windows(4)
            .position(|bytes| bytes == b"covr")
            .unwrap();
        let mut reader = CountingReader {
            reader: std::io::Cursor::new(&fixture),
            forbidden: (covr + 4) as u64..(covr - 4 + artwork.len()) as u64,
            bytes_read: 0,
            seeks: 0,
        };
        // Content, not the extension, must choose the MP4 path.
        let query = read_audio(&mut reader, Path::new("disguised.wav")).unwrap();
        assert_eq!(query.title.as_deref(), Some("A Song (Live)"));
        assert_eq!(query.artists, ["Alice", "Bob"]);
        assert_eq!(query.album.as_deref(), Some("Live Album"));
        assert_eq!(query.duration, Some(1.024));
        assert!(query.keywords.is_empty());
        assert!(reader.bytes_read < 4096);
        assert!(reader.seeks > 0);
        let directory = tempfile::tempdir().unwrap();
        for name in ["track.m4a", "disguised.wav"] {
            let path = directory.path().join(name);
            fs::write(&path, &fixture).unwrap();
            assert_eq!(read_file(&path).unwrap().duration, Some(1.024));
            assert_eq!(fs::read(&path).unwrap(), fixture);
        }
        let wav_path = directory.path().join("actually-wav.m4a");
        fs::write(&wav_path, wav(Some(&id3()))).unwrap();
        assert_eq!(read_file(&wav_path).unwrap().duration, Some(1.0));
    }

    #[test]
    fn mp4_metadata_accepts_bounded_extended_and_zero_sized_tag_atoms() {
        let mut data = 1_u32.to_be_bytes().to_vec();
        data.extend_from_slice(b"data");
        data.extend_from_slice(&29_u64.to_be_bytes()); // 16 header + 8 type/locale + 5 text.
        data.extend_from_slice(&1_u32.to_be_bytes());
        data.extend_from_slice(&[0; 4]);
        data.extend_from_slice(b"Title");
        let mut title = 0_u32.to_be_bytes().to_vec(); // Ends at parent, not file.
        title.extend_from_slice(b"\xa9nam");
        title.extend(data);
        let mut ilst = 1_u32.to_be_bytes().to_vec();
        ilst.extend_from_slice(b"ilst");
        ilst.extend_from_slice(&(16 + title.len() as u64).to_be_bytes());
        ilst.extend(title);
        let mut fixture = mp4_fixture(&ilst);
        fixture.extend(mp4_atom(b"free", b"not part of zero-sized title"));
        let query = read_audio(std::io::Cursor::new(fixture), Path::new("track.m4a")).unwrap();
        assert_eq!(query.title.as_deref(), Some("Title"));
        assert_eq!(query.duration, Some(1.024));
    }

    #[test]
    fn mp4_rejects_malformed_bounds_text_and_excessive_entries() {
        let cases = [
            vec![0, 0, 0, 7, b'f', b'r', b'e', b'e'],
            vec![0, 0, 0, 1, b'f', b'r', b'e', b'e'], // Missing extended size.
            [
                1_u32.to_be_bytes().to_vec(),
                b"free".to_vec(),
                15_u64.to_be_bytes().to_vec(),
            ]
            .concat(),
            [
                1_u32.to_be_bytes().to_vec(),
                b"free".to_vec(),
                u64::MAX.to_be_bytes().to_vec(),
            ]
            .concat(),
            vec![0, 0, 0, 40, b'f', b'r', b'e', b'e'], // Beyond parent/file.
            vec![0; 7],
            mp4_atom(b"\xa9nam", &mp4_atom(b"data", &[0; 7])),
            mp4_atom(b"\xa9nam", &mp4_data(1, &[0xff])),
            mp4_atom(b"\xa9nam", &mp4_data(2, &[0])),
            mp4_atom(b"\xa9nam", &mp4_data(2, &[0xd8, 0x00])),
            mp4_atom(b"\xa9nam", &mp4_data(1, &vec![b'a'; 1025])),
            mp4_atom(b"\xa9nam", &mp4_data(2, &vec![0; 2050])),
            mp4_atom(b"\xa9nam", &mp4_data(21, b"wrong type")),
            mp4_atom(b"\xa9ART", &mp4_data(1, b"Artist").repeat(65)),
            mp4_atom(b"free", &[]).repeat(MP4_ATOM_LIMIT),
        ];
        for content in cases {
            let fixture = mp4_fixture(&mp4_atom(b"ilst", &content));
            assert!(read_audio(std::io::Cursor::new(fixture), Path::new("bad.m4a")).is_err());
        }
        let mut metadata = Mp4Metadata::default();
        assert!(metadata
            .walk(
                &mut std::io::Cursor::new(Vec::<u8>::new()),
                0,
                0,
                Mp4Level::Meta,
                MP4_DEPTH_LIMIT + 1
            )
            .is_err());
        let mut metadata = Mp4Metadata {
            text_bytes: TAG_ALLOCATION_LIMIT as u64,
            ..Mp4Metadata::default()
        };
        let data = mp4_data(1, b"x");
        assert!(metadata
            .read_text(&mut &data[8..], 8, data.len() as u64, *b"\xa9nam")
            .is_err());
        // Unsupported structural encodings must be errors, not fake durations.
        let mut fixture = mp4_fixture(&mp4_atom(b"ilst", &mp4_identity()));
        let moov = fixture
            .windows(4)
            .position(|bytes| bytes == b"moov")
            .unwrap();
        fixture[moov - 4..moov].copy_from_slice(&[0; 4]);
        assert!(read_audio(std::io::Cursor::new(fixture), Path::new("zero.m4a")).is_err());
    }

    #[test]
    fn mp4_rejects_truncated_media_header_before_reading_sibling_as_duration() {
        let mut fixture = mp4_fixture(&mp4_atom(b"ilst", &mp4_identity()));
        let atom = |bytes: &[u8], kind: &[u8; 4]| {
            bytes.windows(4).position(|value| value == kind).unwrap() - 4
        };
        let mdhd = atom(&fixture, b"mdhd");
        for kind in [b"moov", b"trak", b"mdia", b"mdhd"] {
            let position = atom(&fixture, kind);
            let size = u32::from_be_bytes(fixture[position..position + 4].try_into().unwrap());
            fixture[position..position + 4].copy_from_slice(&(size - 8).to_be_bytes());
        }
        fixture.drain(mdhd + 24..mdhd + 32);
        let result = read_audio(std::io::Cursor::new(fixture), Path::new("short.m4a"));
        assert!(
            result.is_err(),
            "accepted malformed media header: {result:?}"
        );
    }

    #[test]
    fn mp4_validates_media_header_versions_and_sample_table_bounds() {
        for (version, length, valid) in [
            (0, 24, true),
            (0, 23, false),
            (1, 36, true),
            (1, 35, false),
            (2, 36, false),
        ] {
            let mut content = vec![0; length];
            content[0] = version;
            let fixture = mp4_atom(
                b"moov",
                &mp4_atom(b"trak", &mp4_atom(b"mdia", &mp4_atom(b"mdhd", &content))),
            );
            assert_eq!(
                Mp4Metadata::read(&mut std::io::Cursor::new(fixture)).is_ok(),
                valid
            );
        }
        let fixture = mp4_fixture(&mp4_atom(b"ilst", &mp4_identity()));
        let stts = fixture
            .windows(4)
            .position(|value| value == b"stts")
            .unwrap()
            - 4;
        for (offset, bytes) in [
            (stts + 12, 2_u32.to_be_bytes()),
            (stts + 8, 1_u32.to_be_bytes()),
        ] {
            let mut invalid = fixture.clone();
            invalid[offset..offset + 4].copy_from_slice(&bytes);
            assert!(read_audio(std::io::Cursor::new(invalid), Path::new("table.m4a")).is_err());
        }
    }

    #[test]
    fn mp4_properties_guard_rejects_crossing_into_metadata_payloads() {
        let mut cursor = std::io::Cursor::new(vec![0; 32]);
        let mut reader = Mp4PropertiesReader {
            reader: &mut cursor,
            skipped: &[8..24],
        };
        let mut bytes = [0; 16];
        assert_eq!(reader.read(&mut bytes).unwrap(), 8);
        assert_eq!(
            reader.read(&mut bytes).unwrap_err().kind(),
            std::io::ErrorKind::InvalidData
        );
        reader.seek(SeekFrom::Start(24)).unwrap();
        assert_eq!(reader.read(&mut bytes).unwrap(), 8);
    }

    #[cfg(unix)]
    #[test]
    fn reads_non_utf8_audio_path() {
        use std::os::unix::ffi::OsStringExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory
            .path()
            .join(std::ffi::OsString::from_vec(b"song\xff.wav".to_vec()));
        fs::write(&path, wav(Some(&id3()))).unwrap();
        assert_eq!(
            read_file(&path).unwrap().title.as_deref(),
            Some("A Song (Live)")
        );
    }
}

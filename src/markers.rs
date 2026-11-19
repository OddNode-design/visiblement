//! Markers stored in WAV files: RIFF cue points (+ LIST/adtl labels), and the XMP markers
//! After Effects / Premiere / Audition write in the `_PMX` chunk. Turned into an FFmetadata
//! chapters file that mpv (and uosc's timeline) display.

use std::path::Path;

#[derive(Debug, PartialEq)]
pub struct Marker {
    pub time: f64,
    pub name: String,
}

pub fn is_wav(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    [".wav", ".wave", ".bwf"].iter().any(|e| p.ends_with(e))
}

pub fn read_wav_markers(path: &Path) -> Vec<Marker> {
    std::fs::read(path).map(|b| parse_wav(&b)).unwrap_or_default()
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?))
}

/// Walks the RIFF chunks; malformed or truncated files just yield fewer markers.
pub fn parse_wav(b: &[u8]) -> Vec<Marker> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WAVE" {
        return Vec::new();
    }
    let (mut rate, mut cues, mut labels, mut xmp) = (0u32, Vec::new(), Vec::new(), None);
    let mut i = 12;
    while i + 8 <= b.len() {
        let id = &b[i..i + 4];
        let size = u32_at(b, i + 4).unwrap_or(0) as usize;
        let data = &b[i + 8..(i + 8).saturating_add(size).min(b.len())];
        match id {
            b"fmt " => rate = u32_at(data, 4).unwrap_or(0),
            b"cue " => {
                let n = u32_at(data, 0).unwrap_or(0) as usize;
                for k in 0..n {
                    // cue point: id, position, chunk id, chunk start, block start, sample offset
                    let base = 4 + k * 24;
                    if let (Some(cid), Some(offset)) = (u32_at(data, base), u32_at(data, base + 20)) {
                        cues.push((cid, offset));
                    }
                }
            }
            b"LIST" if data.get(0..4) == Some(b"adtl") => {
                let mut j = 4;
                while j + 8 <= data.len() {
                    let sub = &data[j..j + 4];
                    let len = u32_at(data, j + 4).unwrap_or(0) as usize;
                    let body = &data[j + 8..(j + 8 + len).min(data.len())];
                    // "labl" is the marker's name, "note" its comment: the name wins when both exist.
                    if (sub == b"labl" || sub == b"note") && body.len() >= 4 {
                        let text = String::from_utf8_lossy(&body[4..]).trim_end_matches('\0').trim().to_string();
                        if !text.is_empty() {
                            labels.push((u32_at(body, 0).unwrap_or(0), sub == b"labl", text));
                        }
                    }
                    j += 8 + len + (len & 1);
                }
            }
            b"_PMX" | b"XMP " | b"iXML" => xmp = Some(String::from_utf8_lossy(data).into_owned()),
            _ => {}
        }
        i += 8 + size + (size & 1);
    }

    let mut out: Vec<Marker> = if rate > 0 {
        cues.iter()
            .map(|&(cid, offset)| Marker {
                time: offset as f64 / rate as f64,
                name: labels
                    .iter()
                    .filter(|(l, _, _)| *l == cid)
                    .max_by_key(|(_, is_name, _)| *is_name)
                    .map(|(_, _, t)| t.clone())
                    .unwrap_or_default(),
            })
            .collect()
    } else {
        Vec::new()
    };
    if out.is_empty() {
        out = xmp.map(|x| parse_xmp_markers(&x, rate)).unwrap_or_default();
    }
    out.sort_by(|a, b| a.time.total_cmp(&b.time));
    out
}

/// Value of `xmpDM:<key>`, written either as an attribute or as a child element.
fn xmp_value<'a>(s: &'a str, key: &str) -> Option<&'a str> {
    let attr = format!("xmpDM:{key}=\"");
    if let Some(p) = s.find(&attr) {
        let rest = &s[p + attr.len()..];
        return rest.find('"').map(|e| &rest[..e]);
    }
    let open = format!("<xmpDM:{key}>");
    let p = s.find(&open)? + open.len();
    let rest = &s[p..];
    rest.find('<').map(|e| &rest[..e])
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#xA;", " ")
        .replace("&amp;", "&")
}

/// Marker times are counted in units of the track's frame rate: "f48000" (samples) or "f30000s1001".
fn parse_rate(r: &str) -> Option<f64> {
    let r = r.strip_prefix('f')?;
    let (num, den) = r.split_once('s').unwrap_or((r, "1"));
    let (n, d): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (n > 0.0 && d > 0.0).then(|| n / d)
}

pub fn parse_xmp_markers(x: &str, sample_rate: u32) -> Vec<Marker> {
    let rate = xmp_value(x, "frameRate").and_then(parse_rate).unwrap_or(sample_rate as f64);
    if rate <= 0.0 {
        return Vec::new();
    }
    let Some(start) = x.find("xmpDM:markers") else { return Vec::new() };
    let section = &x[start..];
    let section = &section[..section.find("</xmpDM:markers>").unwrap_or(section.len())];
    section
        .split("<rdf:li")
        .skip(1)
        .filter_map(|li| {
            let t: f64 = xmp_value(li, "startTime")?.trim().parse().ok()?;
            // After Effects keeps the marker text in "comment"; Premiere / Audition use "name".
            let name = [xmp_value(li, "name"), xmp_value(li, "comment")]
                .into_iter()
                .flatten()
                .map(|v| xml_unescape(v.trim()))
                .find(|v| !v.is_empty())
                .unwrap_or_default();
            Some(Marker { time: t / rate, name })
        })
        .collect()
}

/// FFmetadata chapters file, which mpv reads through `chapters-file`.
pub fn to_ffmetadata(markers: &[Marker]) -> String {
    let mut s = String::from(";FFMETADATA1\n");
    for (i, m) in markers.iter().enumerate() {
        let ms = (m.time * 1000.0).round() as i64;
        let title = if m.name.is_empty() { format!("Marqueur {}", i + 1) } else { m.name.clone() };
        // FFmetadata escapes: = ; # \ and newlines.
        let title: String = title.chars().flat_map(|c| match c {
            '=' | ';' | '#' | '\\' => vec!['\\', c],
            '\n' | '\r' => vec![' '],
            _ => vec![c],
        }).collect();
        s.push_str(&format!("[CHAPTER]\nTIMEBASE=1/1000\nSTART={ms}\nEND={ms}\ntitle={title}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = id.to_vec();
        v.extend((data.len() as u32).to_le_bytes());
        v.extend(data);
        if data.len() % 2 == 1 {
            v.push(0);
        }
        v
    }

    fn wav(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = b"RIFF".to_vec();
        v.extend(((body.len() + 4) as u32).to_le_bytes());
        v.extend(b"WAVE");
        v.extend(body);
        v
    }

    fn fmt(rate: u32) -> Vec<u8> {
        let mut d = vec![1, 0, 2, 0];
        d.extend(rate.to_le_bytes());
        d.extend([0; 8]);
        chunk(b"fmt ", &d)
    }

    #[test]
    fn cue_points_with_labels() {
        let mut cue = 2u32.to_le_bytes().to_vec();
        for (id, off) in [(1u32, 96_000u32), (2, 24_000)] {
            cue.extend(id.to_le_bytes());
            cue.extend(0u32.to_le_bytes());
            cue.extend(b"data");
            cue.extend([0; 8]);
            cue.extend(off.to_le_bytes());
        }
        let mut labl = 1u32.to_le_bytes().to_vec();
        labl.extend(b"Drop\0");
        let mut adtl = b"adtl".to_vec();
        adtl.extend(chunk(b"labl", &labl));
        let file = wav(&[fmt(48_000), chunk(b"cue ", &cue), chunk(b"LIST", &adtl), chunk(b"data", &[0; 4])]);
        let m = parse_wav(&file);
        assert_eq!(m, vec![Marker { time: 0.5, name: String::new() }, Marker { time: 2.0, name: "Drop".into() }]);
    }

    #[test]
    fn after_effects_xmp_markers() {
        let xmp = r#"<x:xmpmeta><rdf:Description xmpDM:frameRate="f48000">
            <xmpDM:markers><rdf:Seq>
            <rdf:li rdf:parseType="Resource"><xmpDM:startTime>144000</xmpDM:startTime><xmpDM:name>Refrain</xmpDM:name></rdf:li>
            <rdf:li xmpDM:startTime="48000" xmpDM:name="Intro"/>
            </rdf:Seq></xmpDM:markers></rdf:Description></x:xmpmeta>"#;
        let file = wav(&[fmt(44_100), chunk(b"_PMX", xmp.as_bytes()), chunk(b"data", &[0; 2])]);
        let m = parse_wav(&file);
        assert_eq!(m, vec![Marker { time: 1.0, name: "Intro".into() }, Marker { time: 3.0, name: "Refrain".into() }]);
    }

    #[test]
    fn after_effects_comment_is_the_name() {
        let xmp = r#"<xmpDM:Tracks><rdf:li xmpDM:frameRate="f48000"><xmpDM:markers><rdf:Seq>
            <rdf:li rdf:parseType="Resource"><xmpDM:startTime>96000</xmpDM:startTime><xmpDM:comment>Coup de c&amp;ymbale</xmpDM:comment></rdf:li>
            <rdf:li xmpDM:startTime="0" xmpDM:name="" xmpDM:comment="Début"/>
            </rdf:Seq></xmpDM:markers></rdf:li></xmpDM:Tracks>"#;
        let m = parse_xmp_markers(xmp, 44_100);
        assert_eq!(m.iter().map(|m| (m.time, m.name.as_str())).collect::<Vec<_>>(), vec![(2.0, "Coup de c&ymbale"), (0.0, "Début")]);
    }

    #[test]
    fn garbage_is_harmless() {
        assert!(parse_wav(b"RIFF\xff\xff\xff\xffWAVEcue \xff\xff\xff\xff").is_empty());
        assert!(parse_wav(b"nope").is_empty());
        assert_eq!(parse_rate("f30000s1001").map(|r| (r * 1000.0).round()), Some(29970.0));
    }

    #[test]
    fn ffmetadata_escapes_titles() {
        let s = to_ffmetadata(&[Marker { time: 1.5, name: "a=b;c".into() }, Marker { time: 2.0, name: String::new() }]);
        assert!(s.contains("START=1500\n"));
        assert!(s.contains("title=a\\=b\\;c\n"));
        assert!(s.contains("title=Marqueur 2\n"));
    }
}

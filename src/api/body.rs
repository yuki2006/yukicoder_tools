//! リクエストボディの組み立てと圧縮。
//!
//! サーバは `Content-Encoding: gzip` のリクエストボディを展開する。展開は
//! multipart のパースより前に走るので、JSON でも multipart でも同じように
//! 圧縮して送れる。

use std::io::Write;

use anyhow::{bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

/// これより小さいボディは圧縮しない。
///
/// gzip のヘッダとフッタで 20 バイト前後増えるうえ、テストケースは中央値が
/// 1KB 程度で、小さいものを圧縮しても得がない。
const MIN_COMPRESS_BYTES: usize = 1024;

/// 送るボディ。圧縮したかどうかを持つ。
pub struct Body {
    pub bytes: Vec<u8>,
    pub gzipped: bool,
}

impl Body {
    /// 大きければ gzip する。縮まなかったときは元のまま送る。
    pub fn new(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < MIN_COMPRESS_BYTES {
            return Ok(Self {
                bytes,
                gzipped: false,
            });
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&bytes)
            .context("リクエストボディを圧縮できませんでした")?;
        let compressed = encoder
            .finish()
            .context("リクエストボディを圧縮できませんでした")?;
        if compressed.len() >= bytes.len() {
            return Ok(Self {
                bytes,
                gzipped: false,
            });
        }
        Ok(Self {
            bytes: compressed,
            gzipped: true,
        })
    }
}

/// `multipart/form-data` の 1 つのパート。
pub enum Part {
    /// ただの値。
    Text { name: String, value: String },
    /// ファイル。サーバ上の名前は `filename` で決まる。
    File {
        name: String,
        filename: String,
        content: Vec<u8>,
    },
}

impl Part {
    pub fn text(name: &str, value: impl Into<String>) -> Self {
        Part::Text {
            name: name.to_string(),
            value: value.into(),
        }
    }

    pub fn file(name: &str, filename: impl Into<String>, content: Vec<u8>) -> Self {
        Part::File {
            name: name.to_string(),
            filename: filename.into(),
            content,
        }
    }

    fn body(&self) -> &[u8] {
        match self {
            Part::Text { value, .. } => value.as_bytes(),
            Part::File { content, .. } => content,
        }
    }
}

/// `multipart/form-data` のボディを組み立てる。境界文字列も返す。
///
/// reqwest の multipart はストリームとして送られるため、gzip を掛けるには
/// 自分でバイト列を作る必要がある。
///
/// ファイル名は `Content-Disposition` ヘッダに引用符で埋め込むので、ヘッダを
/// 壊す文字 (引用符・バックスラッシュ・改行などの制御文字と、非 ASCII) は
/// 弾く。これはサーバの命名規則の写しではなく、エスケープを不要にするための
/// クライアント自身の安全条件 (命名規則の検証は呼び出し元が行う)。
pub fn multipart(parts: &[Part]) -> Result<(String, Vec<u8>)> {
    for part in parts {
        if let Part::File { filename, .. } = part {
            if filename.is_empty()
                || !filename
                    .chars()
                    .all(|c| c.is_ascii_graphic() && c != '"' && c != '\\')
            {
                bail!("multipart に使えないファイル名です: {filename}");
            }
        }
    }

    let boundary = pick_boundary(parts);
    let mut body = Vec::new();
    for part in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match part {
            Part::Text { name, .. } => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
            }
            Part::File { name, filename, .. } => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(b"Content-Type: text/plain\r\n\r\n");
            }
        }
        body.extend_from_slice(part.body());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((boundary, body))
}

/// 中身に現れない境界文字列を選ぶ。
fn pick_boundary(parts: &[Part]) -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for attempt in 0..64u128 {
        let candidate = format!("yukicodertools{:032x}", seed.wrapping_add(attempt));
        if !parts
            .iter()
            .any(|part| contains(part.body(), candidate.as_bytes()))
        {
            return candidate;
        }
    }
    // ここに来るのは、試したすべての候補が中身に含まれていた場合だけ。
    format!("yukicodertools{seed:032x}zzzz")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn small_bodies_are_not_compressed() {
        let body = Body::new(b"short".to_vec()).unwrap();
        assert!(!body.gzipped);
        assert_eq!(body.bytes, b"short");
    }

    #[test]
    fn large_bodies_are_compressed_and_round_trip() {
        let original = "1 2 3\n".repeat(1000).into_bytes();
        let body = Body::new(original.clone()).unwrap();

        assert!(body.gzipped);
        assert!(body.bytes.len() < original.len());

        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&body.bytes[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, original);
    }

    /// 圧縮して増えるだけの内容は、そのまま送る。
    #[test]
    fn incompressible_bodies_stay_raw() {
        let original: Vec<u8> = (0..4096u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let body = Body::new(original.clone()).unwrap();
        if body.gzipped {
            assert!(body.bytes.len() < original.len());
        } else {
            assert_eq!(body.bytes, original);
        }
    }

    #[test]
    fn multipart_body_has_the_expected_shape() {
        let parts = vec![
            Part::file("newfiles", "1.txt", b"1\n".to_vec()),
            Part::file("newfiles", "2.txt", b"2\n".to_vec()),
        ];
        let (boundary, body) = multipart(&parts).unwrap();
        let text = String::from_utf8(body).unwrap();

        assert_eq!(
            text,
            format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"newfiles\"; filename=\"1.txt\"\r\n\
                 Content-Type: text/plain\r\n\r\n\
                 1\n\r\n\
                 --{boundary}\r\n\
                 Content-Disposition: form-data; name=\"newfiles\"; filename=\"2.txt\"\r\n\
                 Content-Type: text/plain\r\n\r\n\
                 2\n\r\n\
                 --{boundary}--\r\n"
            )
        );
    }

    /// 境界文字列が中身に現れると multipart が壊れる。
    #[test]
    fn boundary_does_not_appear_in_the_content() {
        let parts = vec![Part::file("newfiles", "1.txt", b"a\n".to_vec())];
        let (boundary, _) = multipart(&parts).unwrap();
        assert!(!contains(b"a\n", boundary.as_bytes()));
    }

    /// 提出は lang と source をテキストのパートで送る。
    #[test]
    fn text_parts_have_no_filename() {
        let parts = vec![
            Part::text("lang", "cpp23"),
            Part::text("source", "int main() {}"),
        ];
        let (boundary, body) = multipart(&parts).unwrap();
        let text = String::from_utf8(body).unwrap();

        assert_eq!(
            text,
            format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"lang\"\r\n\r\n\
                 cpp23\r\n\
                 --{boundary}\r\n\
                 Content-Disposition: form-data; name=\"source\"\r\n\r\n\
                 int main() {{}}\r\n\
                 --{boundary}--\r\n"
            )
        );
    }

    /// ファイル名は Content-Disposition ヘッダに引用符で埋め込むので、
    /// ヘッダを壊す文字だけを弾く (命名規則の検証は呼び出し元の仕事)。
    #[test]
    fn rejects_names_that_break_the_header() {
        for ok in ["case-01.txt", "1_sample.txt"] {
            let parts = vec![Part::file("newfiles", ok, b"a".to_vec())];
            assert!(multipart(&parts).is_ok(), "{ok}");
        }
        for bad in ["a\"b.txt", "a\\b.txt", "a\r\nb.txt", "テスト.txt", ""] {
            let parts = vec![Part::file("newfiles", bad, b"a".to_vec())];
            assert!(multipart(&parts).is_err(), "{bad:?}");
        }
    }
}

//! Image decoding and unauthenticated HTTP, independent of Git and UI layout.

use std::{
    collections::VecDeque,
    io::{Cursor, Read as _},
    net::{IpAddr, SocketAddr, ToSocketAddrs as _},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

use url::Url;

const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const CACHE_ENTRIES: usize = 64;
const CACHE_TTL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The encoded image plus its logical size, which need not match its pixels.
#[derive(Clone)]
pub(crate) struct DocumentImage {
    pub(crate) image: Arc<gpui::Image>,
    pub(crate) size: gpui::Size<f32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ImageError {
    InvalidUrl,
    Network,
    TooLarge,
    Unsupported,
}

impl ImageError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::InvalidUrl => "Unsupported image URL",
            Self::Network => "Image unavailable",
            Self::TooLarge => "Image exceeds preview limits",
            Self::Unsupported => "Unsupported or invalid image",
        }
    }
}

/// Both local and remote images use the same decoder. HTTP content types can
/// identify SVG without an extension; raster signatures decide their format.
pub(crate) fn decode(bytes: &[u8], media_type: &str) -> Result<DocumentImage, ImageError> {
    if bytes.len() > MAX_BYTES {
        return Err(ImageError::TooLarge);
    }
    let media_type = media_type.split(';').next().unwrap_or_default().trim();
    if media_type.eq_ignore_ascii_case("image/svg+xml") || media_type.eq_ignore_ascii_case("svg") {
        let svg = crate::svg::image_from_bytes(bytes).map_err(|_| ImageError::Unsupported)?;
        return Ok(DocumentImage {
            image: svg.image,
            size: svg.size,
        });
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ImageError::Unsupported)?;
    let format = match reader.format() {
        Some(image::ImageFormat::Png) => gpui::ImageFormat::Png,
        Some(image::ImageFormat::Jpeg) => gpui::ImageFormat::Jpeg,
        Some(image::ImageFormat::Gif) => gpui::ImageFormat::Gif,
        Some(image::ImageFormat::WebP) => gpui::ImageFormat::Webp,
        Some(image::ImageFormat::Bmp) => gpui::ImageFormat::Bmp,
        Some(image::ImageFormat::Tiff) => gpui::ImageFormat::Tiff,
        _ => return Err(ImageError::Unsupported),
    };
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| ImageError::Unsupported)?;
    if width == 0
        || height == 0
        || width > 16384
        || height > 16384
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(ImageError::TooLarge);
    }
    // Reject truncated/corrupt files here, so the UI can display alt text
    // instead of handing undecodable bytes to GPUI's asynchronous image cache.
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ImageError::Unsupported)?;
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    reader.decode().map_err(|_| ImageError::Unsupported)?;
    Ok(DocumentImage {
        image: Arc::new(gpui::Image::from_bytes(format, bytes.to_vec())),
        size: gpui::size(dimension(width), dimension(height)),
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "validated image dimensions are small enough for f32"
)]
fn dimension(value: u32) -> f32 {
    value as f32
}

fn image_url(value: &str) -> Result<Url, ImageError> {
    let mut url = Url::parse(value).map_err(|_| ImageError::InvalidUrl)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(ImageError::InvalidUrl);
    }
    url.set_fragment(None);
    Ok(url)
}

type CachedResult = Result<DocumentImage, ImageError>;
#[derive(Default)]
struct ImageCache {
    entries: VecDeque<(String, Instant, CachedResult)>,
    bytes: usize,
}

impl ImageCache {
    fn get(&mut self, url: &str) -> Option<CachedResult> {
        let index = self.entries.iter().position(|(key, _, _)| key == url)?;
        let entry = self.entries.remove(index)?;
        let ttl = if entry.2.is_ok() {
            CACHE_TTL
        } else {
            Duration::from_secs(5)
        };
        if entry.1.elapsed() > ttl {
            self.bytes -= result_bytes(&entry.2);
            return None;
        }
        let result = entry.2.clone();
        self.entries.push_back(entry);
        Some(result)
    }

    fn insert(&mut self, url: String, result: CachedResult) {
        if let Some(index) = self.entries.iter().position(|(key, _, _)| *key == url)
            && let Some((_, _, old)) = self.entries.remove(index)
        {
            self.bytes -= result_bytes(&old);
        }
        self.bytes += result_bytes(&result);
        self.entries.push_back((url, Instant::now(), result));
        while self.entries.len() > CACHE_ENTRIES || self.bytes > CACHE_BYTES {
            if let Some((_, _, result)) = self.entries.pop_front() {
                self.bytes -= result_bytes(&result);
            }
        }
    }
}

fn result_bytes(result: &CachedResult) -> usize {
    result.as_ref().map_or(0, |image| image.image.bytes.len())
}

/// Fetches on a background worker. The cache lock never spans I/O or decoding.
/// Requests carry no GitHub token, cookies, or repository credentials.
pub(crate) fn fetch(reference: &str) -> CachedResult {
    static CACHE: LazyLock<Mutex<ImageCache>> = LazyLock::new(Mutex::default);
    fetch_with(reference, &CACHE, download)
}

fn fetch_with(
    reference: &str,
    cache: &Mutex<ImageCache>,
    transport: impl FnOnce(&Url) -> Result<(Vec<u8>, String), ImageError>,
) -> CachedResult {
    let url = image_url(reference)?;
    if let Some(result) = cache
        .lock()
        .ok()
        .and_then(|mut cache| cache.get(url.as_str()))
    {
        return result;
    }
    let result = transport(&url).and_then(|(bytes, media_type)| decode(&bytes, &media_type));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(url.into(), result.clone());
    }
    result
}

fn download(url: &Url) -> Result<(Vec<u8>, String), ImageError> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .resolver(public_addresses)
        .build();
    let started = Instant::now();
    let mut url = url.clone();
    for _ in 0..=5 {
        let remaining = REQUEST_TIMEOUT
            .checked_sub(started.elapsed())
            .ok_or(ImageError::Network)?;
        let response = agent
            .get(url.as_str())
            .set("User-Agent", "Sourcefour")
            .set("Accept", "image/svg+xml,image/*;q=0.9")
            .timeout(remaining)
            .call()
            .map_err(|_| ImageError::Network)?;
        if (300..400).contains(&response.status()) {
            let target = response.header("Location").ok_or(ImageError::Network)?;
            let target = url.join(target).map_err(|_| ImageError::InvalidUrl)?;
            let target = image_url(target.as_str())?;
            if url.scheme() == "https" && target.scheme() != "https" {
                return Err(ImageError::InvalidUrl);
            }
            url = target;
            continue;
        }
        let media_type = response
            .header("Content-Type")
            .unwrap_or_default()
            .to_owned();
        if response
            .header("Content-Length")
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(ImageError::TooLarge);
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ImageError::Network)?;
        if bytes.len() > MAX_BYTES {
            return Err(ImageError::TooLarge);
        }
        return Ok((bytes, media_type));
    }
    Err(ImageError::Network)
}

/// A repository can contain untrusted URLs. Resolve once at the connection
/// boundary so redirects and DNS changes cannot reach local services.
fn public_addresses(netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
    let addresses: Vec<_> = netloc
        .to_socket_addrs()?
        .filter(|address| public_ip(address.ip()))
        .collect();
    if addresses.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "Image host has no public address",
        ));
    }
    Ok(addresses)
}

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, _, _] = ip.octets();
            !ip.is_private()
                && !ip.is_loopback()
                && !ip.is_link_local()
                && !ip.is_broadcast()
                && !ip.is_documentation()
                && a != 0
                && a < 224
                && !(a == 100 && (64..128).contains(&b))
                && !(a == 198 && (b == 18 || b == 19))
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(ip));
            }
            // Global unicast only; excludes loopback, local, link-local,
            // multicast, and IPv4-compatible addresses.
            let first = ip.segments()[0];
            (0x2000..0x4000).contains(&first) && !(first == 0x2001 && ip.segments()[1] == 0x0db8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BADGE: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" width="78" height="20"><path fill="red" d="M0 0h78v20H0z"/></svg>"#;

    #[test]
    fn extensionless_badge_uses_content_type_and_keeps_logical_size() {
        let image = fetch_with(
            "https://example.com/badge?logo=rust",
            &Mutex::default(),
            |url| {
                assert_eq!(url.query(), Some("logo=rust"));
                Ok((BADGE.to_vec(), "image/svg+xml; charset=utf-8".into()))
            },
        )
        .expect("badge renders");
        assert_eq!(image.size, gpui::size(78.0, 20.0));
        let pixels = image::load_from_memory(&image.image.bytes).expect("encoded PNG");
        assert_eq!((pixels.width(), pixels.height()), (156, 40));
    }

    #[test]
    fn repeated_urls_share_cached_images_and_failures() {
        let cache = Mutex::default();
        let first = fetch_with("https://example.com/badge", &cache, |_| {
            Ok((BADGE.to_vec(), "svg".into()))
        })
        .expect("image");
        let second = fetch_with("https://example.com/badge#fragment", &cache, |_| {
            panic!("cache hit must not request again")
        })
        .expect("cached");
        assert!(Arc::ptr_eq(&first.image, &second.image));
        assert_eq!(
            fetch_with("https://example.com/fail", &cache, |_| Err(
                ImageError::Network
            ))
            .err(),
            Some(ImageError::Network)
        );
        assert_eq!(
            fetch_with("https://example.com/fail", &cache, |_| panic!(
                "failure cached"
            ))
            .err(),
            Some(ImageError::Network)
        );
    }

    #[test]
    fn malformed_images_and_unsupported_urls_are_rejected() {
        assert_eq!(
            decode(b"<html>error</html>", "image/svg+xml").err(),
            Some(ImageError::Unsupported)
        );
        assert_eq!(
            decode(b"\x89PNG\r\n\x1a\n", "image/png").err(),
            Some(ImageError::Unsupported)
        );
        assert_eq!(
            decode(&vec![0; MAX_BYTES + 1], "image/png").err(),
            Some(ImageError::TooLarge)
        );
        for url in [
            "file:///etc/passwd",
            "data:image/svg+xml,",
            "https://user:secret@example.com/image",
        ] {
            assert_eq!(
                fetch_with(url, &Mutex::default(), |_| panic!("must not request")).err(),
                Some(ImageError::InvalidUrl)
            );
        }
    }

    #[test]
    fn cache_expires_and_evicts_old_entries() {
        let mut cache = ImageCache::default();
        cache.insert("expired".into(), Err(ImageError::Network));
        cache.entries[0].1 = Instant::now()
            .checked_sub(CACHE_TTL)
            .expect("clock exceeds cache TTL");
        assert!(cache.get("expired").is_none());
        for index in 0..=CACHE_ENTRIES {
            cache.insert(index.to_string(), Err(ImageError::Network));
        }
        assert_eq!(cache.entries.len(), CACHE_ENTRIES);
        assert!(cache.get("0").is_none());
    }

    #[test]
    fn remote_images_cannot_connect_to_local_services() {
        for address in [
            "127.0.0.1",
            "10.0.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                !public_ip(address.parse().expect("IP address")),
                "{address}"
            );
        }
        for address in ["8.8.8.8", "2606:4700:4700::1111"] {
            assert!(public_ip(address.parse().expect("public IP")));
        }
    }
}

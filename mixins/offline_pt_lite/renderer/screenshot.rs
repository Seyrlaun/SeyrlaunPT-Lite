use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const MAX_PIXELS: usize = 64 * 1024 * 1024;
const MAX_DIMENSION: u32 = 32_768;
const HEADER_LIMIT: usize = 1024;
const DENOISE_STRENGTH: f32 = 0.7;
const OIDN_TIMEOUT: Duration = Duration::from_secs(180);
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
pub(super) struct Pixels {
    pub path: PathBuf,
    pub source_size: [u32; 2],
    pub target_size: [u32; 2],
    pub exposure: f32,
    pub beauty: Vec<[f32; 3]>,
    pub albedo: Vec<[f32; 3]>,
    pub normal: Vec<[f32; 3]>,
}
pub(super) fn write(pixels: Pixels) -> Result<[u32; 2], String> {
    write_using(pixels, denoise)
}
fn pixel_count(size: [u32; 2]) -> Result<usize, String> {
    if size.iter().any(|&value| value == 0 || value > MAX_DIMENSION) {
        return Err("screenshot dimensions are outside the supported range".into());
    }
    let count = (size[0] as usize)
        .checked_mul(size[1] as usize)
        .filter(|&count| count <= MAX_PIXELS)
        .ok_or("screenshot pixel count is outside the supported range")?;
    Ok(count)
}
fn validate(pixels: &Pixels) -> Result<(), String> {
    let count = pixel_count(pixels.source_size)?;
    pixel_count(pixels.target_size)?;
    if pixels.beauty.len() != count || pixels.albedo.len() != count
        || pixels.normal.len() != count
    {
        return Err("screenshot beauty/albedo/normal dimensions do not match".into());
    }
    if !pixels.exposure.is_finite() || pixels.exposure < 0.0 {
        return Err("screenshot exposure must be finite and nonnegative".into());
    }
    if pixels.path.file_stem().is_none() {
        return Err("screenshot path must name a file".into());
    }
    Ok(())
}
fn finite(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}
fn sanitize(pixels: &mut Pixels) {
    for color in &mut pixels.beauty {
        *color = color.map(|value| finite(value).max(0.0));
    }
    for color in &mut pixels.albedo {
        *color = color.map(|value| finite(value).clamp(0.0, 1.0));
    }
    for normal in &mut pixels.normal {
        *normal = normal.map(|value| finite(value).clamp(-1.0, 1.0));
    }
}
fn related_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_stem().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}
fn write_using(
    mut pixels: Pixels,
    denoiser: impl FnOnce(&Pixels) -> Result<Vec<[f32; 3]>, String>,
) -> Result<[u32; 2], String> {
    validate(&pixels)?;
    sanitize(&mut pixels);
    let parent = pixels
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("screenshot directory: {error}"))?;
    let raw = related_path(&pixels.path, "-raw.png");
    write_png(&raw, &pixels, &pixels.beauty, Encoding::Beauty)?;
    write_png(&pixels.path, &pixels, &pixels.beauty, Encoding::Beauty)?;
    write_png(
        &related_path(&pixels.path, "-albedo.png"),
        &pixels,
        &pixels.albedo,
        Encoding::Albedo,
    )?;
    write_png(
        &related_path(&pixels.path, "-normal.png"),
        &pixels,
        &pixels.normal,
        Encoding::Normal,
    )?;
    let started = Instant::now();
    let result = denoiser(&pixels)
        .and_then(|denoised| {
            if denoised.len() != pixels.beauty.len()
                || denoised.iter().flatten().any(|value| !value.is_finite())
            {
                return Err("OIDN returned invalid pixels or dimensions".into());
            }
            blend(&mut pixels.beauty, &denoised);
            write_png(&pixels.path, &pixels, &pixels.beauty, Encoding::Beauty)
        });
    let message = match result {
        Ok(()) => {
            "OIDN high quality: 70% denoised + 30% raw in scene-linear light.\n\
            Main/raw PNGs have display exposure and Lite filmic/sRGB applied.\n\
            Albedo/normal PNGs are inspection guides, not direct HDR-denoise inputs.\n"
                .to_owned()
        }
        Err(error) => {
            log::warn!("Offline PT screenshot kept raw fallback: {error}");
            format!(
                "OIDN failed; main PNG is the raw fallback. All four PNGs were retained.\n\
                 {error}\nInspection PNGs are not scene-linear HDR denoiser inputs.\n"
            )
        }
    };
    let message = format!(
        "{message}Denoise workflow elapsed: {:.3} seconds (includes float file I/O).\n",
        started.elapsed().as_secs_f64()
    );
    if let Err(error) = fs::write(related_path(&pixels.path, "-oidn.log"), message) {
        log::warn!("Offline PT screenshot log could not be saved: {error}");
    }
    Ok(pixels.target_size)
}
fn blend(raw: &mut [[f32; 3]], denoised: &[[f32; 3]]) {
    for (raw, denoised) in raw.iter_mut().zip(denoised) {
        for channel in 0..3 {
            raw[channel] = ((raw[channel] as f64 * (1.0 - DENOISE_STRENGTH) as f64)
                + (denoised[channel].max(0.0) as f64 * DENOISE_STRENGTH as f64))
                .min(f32::MAX as f64) as f32;
        }
    }
}
fn filmic(color: [f32; 3], exposure: f32) -> [f32; 3] {
    let color = color
        .map(|value| {
            (finite(value).max(0.0) as f64 * exposure as f64).min(f32::MAX as f64) as f32
        });
    let peak = color[0].max(color[1]).max(color[2]);
    let mapped = if peak > 0.25 {
        let knee = 0.25;
        let denominator = knee * (2.43 * knee + 0.59) + 0.14;
        let at_knee = 0.98 * knee * (2.43 * knee + 0.03) / denominator;
        let slope = 0.98
            * (2.43 * 0.56 * knee * knee + 2.0 * 2.43 * 0.14 * knee + 0.03 * 0.14)
            / (denominator * denominator);
        let headroom = 0.98 - at_knee;
        let width = headroom * 0.35 / slope;
        at_knee + headroom * (1.0 - (1.0 + (peak - knee) / width).powf(-0.35))
    } else {
        0.98 * (peak * (2.43 * peak + 0.03)) / (peak * (2.43 * peak + 0.59) + 0.14)
    };
    color.map(|value| value * (mapped / peak.max(1e-12)))
}
fn srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}
#[derive(Clone, Copy)]
enum Encoding {
    Beauty,
    Albedo,
    Normal,
}
fn encode(color: [f32; 3], encoding: Encoding, exposure: f32) -> [u8; 3] {
    let rgb = match encoding {
        Encoding::Beauty => filmic(color, exposure).map(srgb),
        Encoding::Albedo => color.map(|value| srgb(finite(value).clamp(0.0, 1.0))),
        Encoding::Normal => color.map(|value| finite(value).clamp(-1.0, 1.0) * 0.5 + 0.5),
    };
    rgb.map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
}
fn nearest(position: u32, source: u32, target: u32) -> usize {
    (((2 * position as u64 + 1) * source as u64) / (2 * target as u64))
        .min(source as u64 - 1) as usize
}
struct PendingFile(PathBuf);
impl Drop for PendingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn write_png(
    path: &Path,
    pixels: &Pixels,
    source: &[[f32; 3]],
    encoding: Encoding,
) -> Result<(), String> {
    let [width, height] = pixels.target_size;
    let [source_width, source_height] = pixels.source_size;
    let mut bytes = Vec::with_capacity(pixel_count(pixels.target_size)? * 3);
    for y in 0..height {
        let row = nearest(y, source_height, height) * source_width as usize;
        for x in 0..width {
            let color = source[row + nearest(x, source_width, width)];
            bytes.extend_from_slice(&encode(color, encoding, pixels.exposure));
        }
    }
    let temporary = related_path(
        path,
        &format!(
            ".pt-{}-{}.tmp", std::process::id(), NEXT_TEMP.fetch_add(1,
            Ordering::Relaxed)
        ),
    );
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let pending = PendingFile(temporary);
    let save = || -> Result<(), Box<dyn std::error::Error>> {
        let mut file = BufWriter::new(file);
        {
            let mut encoder = png::Encoder::new(&mut file, width, height);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            if !matches!(encoding, Encoding::Normal) {
                encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
            }
            let mut writer = encoder.write_header()?;
            writer.write_image_data(&bytes)?;
            writer.finish()?;
        }
        file.flush()?;
        drop(file);
        fs::rename(&pending.0, path)?;
        Ok(())
    };
    save().map_err(|error| format!("{}: {error}", path.display()))
}
struct PrivateDirectory(PathBuf);
impl PrivateDirectory {
    fn new() -> Result<Self, String> {
        for _ in 0..32 {
            let time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir()
                .join(
                    format!(
                        "horizon-pt-oidn-{}-{time}-{}", std::process::id(), NEXT_TEMP
                        .fetch_add(1, Ordering::Relaxed)
                    ),
                );
            let mut builder = fs::DirBuilder::new();
            builder.recursive(false);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(format!("private OIDN temporary directory: {error}"));
                }
            }
        }
        Err("could not reserve a private OIDN temporary directory".into())
    }
}
impl Drop for PrivateDirectory {
    fn drop(&mut self) {
        for name in ["beauty.pfm", "albedo.pfm", "normal.pfm", "denoised.pfm"] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
fn write_pfm(path: &Path, size: [u32; 2], pixels: &[[f32; 3]]) -> Result<(), String> {
    if pixel_count(size)? != pixels.len()
        || pixels.iter().flatten().any(|v| !v.is_finite())
    {
        return Err("invalid PFM input pixels".into());
    }
    let save = || -> std::io::Result<()> {
        let mut file = BufWriter::new(File::create(path)?);
        writeln!(file, "PF\n{} {}\n-1.0", size[0], size[1])?;
        let width = size[0] as usize;
        let mut row = Vec::with_capacity(width * 12);
        for y in (0..size[1] as usize).rev() {
            row.clear();
            for pixel in &pixels[y * width..(y + 1) * width] {
                for channel in pixel {
                    row.extend_from_slice(&channel.to_le_bytes());
                }
            }
            file.write_all(&row)?;
        }
        file.flush()
    };
    save().map_err(|error| format!("write private PFM: {error}"))
}
fn header_line(
    reader: &mut impl BufRead,
    consumed: &mut usize,
) -> Result<String, String> {
    loop {
        let remaining = HEADER_LIMIT.saturating_sub(*consumed);
        let mut line = Vec::new();
        let count = (&mut *reader)
            .take(remaining as u64 + 1)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("read PFM header: {error}"))?;
        *consumed += count;
        if count == 0 || count > remaining || line.last() != Some(&b'\n') {
            return Err("PFM header is truncated or exceeds its size limit".into());
        }
        let text = std::str::from_utf8(&line).map_err(|_| "PFM header is not UTF-8")?;
        let text = text.split('#').next().unwrap_or_default().trim();
        if !text.is_empty() {
            return Ok(text.to_owned());
        }
    }
}
fn read_pfm(path: &Path, expected: [u32; 2]) -> Result<Vec<[f32; 3]>, String> {
    let count = pixel_count(expected)?;
    let file = File::open(path).map_err(|error| format!("read OIDN output: {error}"))?;
    let length = file.metadata().map_err(|error| error.to_string())?.len();
    if length > (count * 12 + HEADER_LIMIT) as u64 {
        return Err("OIDN PFM exceeds the expected image size".into());
    }
    let mut reader = BufReader::new(file);
    let mut consumed = 0;
    if header_line(&mut reader, &mut consumed)? != "PF" {
        return Err("OIDN output is not an RGB PFM".into());
    }
    let dimensions = header_line(&mut reader, &mut consumed)?;
    let dimensions = dimensions
        .split_whitespace()
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "invalid PFM dimensions")?;
    if dimensions.as_slice() != expected {
        return Err("OIDN PFM dimensions differ from the captured image".into());
    }
    let scale = header_line(&mut reader, &mut consumed)?
        .parse::<f32>()
        .map_err(|_| "invalid PFM scale")?;
    if !scale.is_finite() || scale == 0.0 {
        return Err("PFM scale must be finite and nonzero".into());
    }
    let mut pixels = vec![[0.0; 3]; count];
    let width = expected[0] as usize;
    let mut row = vec![0; width * 12];
    for y in (0..expected[1] as usize).rev() {
        reader
            .read_exact(&mut row)
            .map_err(|error| format!("truncated OIDN PFM pixels: {error}"))?;
        for (pixel, bytes) in pixels[y * width..(y + 1) * width]
            .iter_mut()
            .zip(row.chunks_exact(12))
        {
            for (channel, bytes) in pixel.iter_mut().zip(bytes.chunks_exact(4)) {
                let bytes: [u8; 4] = bytes
                    .try_into()
                    .map_err(|_| "invalid PFM channel")?;
                *channel = if scale < 0.0 {
                    f32::from_le_bytes(bytes)
                } else {
                    f32::from_be_bytes(bytes)
                } * scale.abs();
                if !channel.is_finite() {
                    return Err("OIDN PFM contains non-finite pixels".into());
                }
            }
        }
    }
    let mut extra = [0];
    if reader.read(&mut extra).map_err(|error| error.to_string())? != 0 {
        return Err("OIDN PFM has unexpected trailing data".into());
    }
    Ok(pixels)
}
fn named(path: &Path, name: &str) -> bool {
    path.file_name()
        .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case(name))
}
fn runtime_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![home.to_owned()];
    if let Some(parent) = home.parent() {
        if named(parent, "instances") {
            if let Some(shared) = parent.parent() {
                roots.push(shared.to_owned());
            }
        } else if named(home, "home") && let Some(container) = parent.parent()
            && (named(container, "ports") || named(container, "horizon-packs"))
            && let Some(shared) = container.parent()
        {
            roots.push(shared.to_owned());
        }
    }
    roots
}
fn official_executable(path: PathBuf) -> Result<PathBuf, String> {
    let expected = format!("oidnDenoise{}", std::env::consts::EXE_SUFFIX);
    if !named(&path, &expected) || !path.is_file() {
        return Err(format!("official {expected} was not found at {}", path.display()));
    }
    path.canonicalize()
        .map_err(|error| format!("could not resolve official OIDN path: {error}"))
}
fn resolve_oidn() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("HORIZON_OIDN_BIN") && !path.is_empty() {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err("HORIZON_OIDN_BIN must be an absolute executable path".into());
        }
        return official_executable(path);
    }
    let home = std::env::var_os("HORIZON_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let executable = format!("oidnDenoise{}", std::env::consts::EXE_SUFFIX);
    if let Some(home) = home {
        for root in runtime_roots(&home) {
            let candidate = root
                .join("native-tools/offline_pt_lite/oidn/bin")
                .join(&executable);
            if candidate.is_file() {
                return official_executable(candidate);
            }
        }
    }
    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join("oidn/bin").join(&executable);
        if candidate.is_file() {
            return official_executable(candidate);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            if directory.as_os_str().is_empty() {
                continue;
            }
            let candidate = directory.join(&executable);
            if candidate.is_file() {
                return official_executable(candidate);
            }
        }
    }
    Err(
        "official oidnDenoise was not found; install the offline runtime or set HORIZON_OIDN_BIN"
            .into(),
    )
}
fn oidn_command(executable: &Path, directory: &Path, device: &OsStr) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("--hdr")
        .arg(directory.join("beauty.pfm"))
        .arg("--alb")
        .arg(directory.join("albedo.pfm"))
        .arg("--nrm")
        .arg(directory.join("normal.pfm"))
        .arg("--output")
        .arg(directory.join("denoised.pfm"))
        .args(["--filter", "RT", "--quality", "high", "--device"])
        .arg(device)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if device.to_string_lossy().eq_ignore_ascii_case("cpu") {
        let threads = std::thread::available_parallelism()
            .map(|count| (count.get() / 2).max(1))
            .unwrap_or(1);
        command.arg("--threads").arg(threads.to_string()).args(["--affinity", "0"]);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    command
}
fn run_owned(mut command: Command, timeout: Duration) -> Result<(), String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("could not start official OIDN: {error}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                return Err(format!("OIDN exited unsuccessfully: {status}"));
            }
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(25));
            }
            outcome => {
                let killed = child.kill();
                let reaped = finish_stop(
                    killed,
                    &mut child,
                    std::process::Child::wait,
                    std::process::Child::try_wait,
                );
                return Err(
                    match outcome {
                        Err(error) => {
                            format!("could not wait for OIDN: {error}; reap: {reaped:?}")
                        }
                        _ => {
                            format!(
                                "OIDN timed out after {} seconds; reap: {reaped:?}", timeout
                                .as_secs()
                            )
                        }
                    },
                );
            }
        }
    }
}
fn finish_stop<C, S>(
    killed: std::io::Result<()>,
    child: &mut C,
    wait: impl FnOnce(&mut C) -> std::io::Result<S>,
    poll: impl FnOnce(&mut C) -> std::io::Result<Option<S>>,
) -> std::io::Result<S> {
    match killed {
        Ok(()) => wait(child),
        Err(error) => {
            match poll(child)? {
                Some(status) => Ok(status),
                None => Err(error),
            }
        }
    }
}
fn denoise_with(
    pixels: &Pixels,
    executable: &Path,
    device: &OsStr,
) -> Result<Vec<[f32; 3]>, String> {
    let directory = PrivateDirectory::new()?;
    for (name, image) in [
        ("beauty.pfm", &pixels.beauty),
        ("albedo.pfm", &pixels.albedo),
        ("normal.pfm", &pixels.normal),
    ] {
        write_pfm(&directory.0.join(name), pixels.source_size, image)?;
    }
    run_owned(oidn_command(executable, &directory.0, device), OIDN_TIMEOUT)?;
    read_pfm(&directory.0.join("denoised.pfm"), pixels.source_size)
}
fn denoise(pixels: &Pixels) -> Result<Vec<[f32; 3]>, String> {
    let executable = resolve_oidn()?;
    let device = std::env::var_os("HORIZON_OIDN_DEVICE")
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| OsString::from("cpu"));
    denoise_with(pixels, &executable, &device)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn capture(path: PathBuf) -> Pixels {
        Pixels {
            path,
            source_size: [2, 2],
            target_size: [4, 4],
            exposure: 2.0,
            beauty: vec![[0.1, 0.2, 0.3], [4.0, 2.0, 1.0], [0.0; 3], [0.4; 3]],
            albedo: vec![[0.25, 0.5, 1.0]; 4],
            normal: vec![[0.0, 0.0, 1.0]; 4],
        }
    }
    fn read_png(path: &Path) -> ([u32; 2], Vec<u8>, bool) {
        let mut reader = png::Decoder::new(BufReader::new(File::open(path).unwrap()))
            .read_info()
            .unwrap();
        let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut bytes).unwrap();
        bytes.truncate(info.buffer_size());
        ([info.width, info.height], bytes, reader.info().srgb.is_some())
    }
    #[test]
    fn pfm_round_trip_preserves_signed_hdr_pixels_and_top_down_rows() {
        let directory = PrivateDirectory::new().unwrap();
        let path = directory.0.join("beauty.pfm");
        let pixels = [[-1.0, 0.5, 16.0], [2.0, 3.0, 4.0], [0.0; 3], [32.0; 3]];
        write_pfm(&path, [2, 2], &pixels).unwrap();
        assert_eq!(read_pfm(& path, [2, 2]).unwrap(), pixels);
        let bytes = fs::read(path).unwrap();
        assert_eq!(& bytes[b"PF\n2 2\n-1.0\n".len()..] [..4], & 0.0_f32.to_le_bytes());
    }
    #[test]
    fn pfm_reads_big_endian_scaled_crlf_and_rejects_malformed_payloads() {
        let directory = PrivateDirectory::new().unwrap();
        let path = directory.0.join("beauty.pfm");
        let mut bytes = b"PF\r\n# comment\r\n1 1\r\n2.0\r\n".to_vec();
        for value in [0.25_f32, -0.5, 8.0] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        fs::write(&path, &bytes).unwrap();
        assert_eq!(read_pfm(& path, [1, 1]).unwrap(), [[0.5, - 1.0, 16.0]]);
        assert!(read_pfm(& path, [2, 1]).is_err());
        bytes.pop();
        fs::write(&path, &bytes).unwrap();
        assert!(read_pfm(& path, [1, 1]).is_err());
        fs::write(&path, b"PF\n1 1\nNaN\n").unwrap();
        assert!(read_pfm(& path, [1, 1]).is_err());
        fs::write(&path, vec![b'x'; HEADER_LIMIT + 1]).unwrap();
        assert!(read_pfm(& path, [1, 1]).is_err());
    }
    #[test]
    fn filmic_matches_lite_long_shoulder_and_srgb_transfer() {
        for peak in [0.0_f32, 0.01, 0.1, 0.25, 0.251, 1.0, 4.0, 64.0] {
            let x = peak as f64;
            let knee = 0.25_f64;
            let d = knee * (2.43 * knee + 0.59) + 0.14;
            let at_knee = 0.98 * knee * (2.43 * knee + 0.03) / d;
            let slope = 0.98
                * (2.43 * 0.56 * knee * knee + 2.0 * 2.43 * 0.14 * knee + 0.03 * 0.14)
                / d.powi(2);
            let headroom = 0.98 - at_knee;
            let reference = if x > knee {
                at_knee
                    + headroom
                        * (1.0
                            - (1.0 + (x - knee) / (headroom * 0.35 / slope)).powf(-0.35))
            } else {
                0.98 * x * (2.43 * x + 0.03) / (x * (2.43 * x + 0.59) + 0.14)
            };
            let result = filmic([peak, peak * 0.5, peak * 0.25], 1.0);
            assert!((result[0] as f64 - reference).abs() < 2e-6);
            assert!((result[1] - result[0] * 0.5).abs() < 1e-6);
        }
        assert_eq!(srgb(0.0), 0.0);
        assert!((srgb(0.003_130_8) - 0.040_449_936).abs() < 1e-7);
        assert!((srgb(1.0) - 1.0).abs() < 1e-6);
        assert!(filmic([f32::MAX; 3], f32::MAX).iter().all(| v | v.is_finite()));
    }
    #[test]
    fn blend_is_seventy_percent_denoised_in_linear_light() {
        let mut raw = [[2.0, 4.0, 8.0]];
        blend(&mut raw, &[[0.0, 2.0, 4.0]]);
        for (actual, expected) in raw[0].into_iter().zip([0.6, 2.6, 5.2]) {
            assert!((actual - expected).abs() < 1e-6);
        }
        assert_eq!(encode([0.25, 0.5, 1.0], Encoding::Albedo, 99.0), [137, 188, 255]);
        assert_eq!(encode([- 1.0, 0.0, 1.0], Encoding::Normal, 99.0), [0, 128, 255]);
        assert_eq!(
            (0..4).map(| x | nearest(x, 2, 4)).collect::< Vec < _ >> (), [0, 0, 1, 1]
        );
    }
    #[test]
    fn invalid_capture_is_rejected_before_output_and_finite_guides_are_sanitized() {
        let directory = PrivateDirectory::new().unwrap();
        let mut pixels = capture(directory.0.join("invalid.png"));
        pixels.exposure = f32::NAN;
        assert!(
            write_using(pixels, | _ | panic!("invalid capture reached OIDN")).is_err()
        );
        assert!(! directory.0.join("invalid.png").exists());
        let mut pixels = capture(directory.0.join("unused.png"));
        pixels.beauty[0] = [f32::INFINITY, -1.0, 8.0];
        pixels.albedo[0] = [f32::NAN, 2.0, -1.0];
        pixels.normal[0] = [-2.0, f32::NAN, 2.0];
        sanitize(&mut pixels);
        assert_eq!(pixels.beauty[0], [0.0, 0.0, 8.0]);
        assert_eq!(pixels.albedo[0], [0.0, 1.0, 0.0]);
        assert_eq!(pixels.normal[0], [- 1.0, 0.0, 1.0]);
        pixels.normal.pop();
        assert!(validate(& pixels).is_err());
        assert!(pixel_count([0, 1]).is_err());
        assert!(pixel_count([MAX_DIMENSION, MAX_DIMENSION]).is_err());
    }
    #[test]
    fn oidn_failure_keeps_exactly_four_pngs_and_log_with_raw_main() {
        let directory = PrivateDirectory::new().unwrap();
        let path = directory.0.join("test image.png");
        assert_eq!(
            write_using(capture(path.clone()), | _ | Err("test OIDN failure".into()))
            .unwrap(), [4, 4]
        );
        assert_eq!(
            fs::read(& path).unwrap(), fs::read(related_path(& path, "-raw.png"))
            .unwrap()
        );
        let (size, bytes, tagged) = read_png(&path);
        assert_eq!(size, [4, 4]);
        assert!(tagged);
        assert_eq!(& bytes[0..3], & bytes[3..6]);
        assert_eq!(
            read_png(& related_path(& path, "-albedo.png")).1[0..3], [137, 188, 255]
        );
        assert_eq!(
            read_png(& related_path(& path, "-normal.png")).1[0..3], [128, 128, 255]
        );
        let files = fs::read_dir(&directory.0)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(files.len(), 5);
        assert!(! related_path(& path, "-denoised.png").exists());
        assert!(
            fs::read_to_string(related_path(& path, "-oidn.log")).unwrap()
            .contains("test OIDN failure")
        );
        for file in files {
            fs::remove_file(file.path()).unwrap();
        }
    }
    #[test]
    fn cpu_command_uses_hdr_guides_high_quality_without_clean_aux_or_shell() {
        let executable = Path::new("runtime with spaces/oidnDenoise.exe");
        let directory = Path::new("private path with spaces");
        let command = oidn_command(executable, directory, OsStr::new("cpu"));
        assert_eq!(command.get_program(), executable.as_os_str());
        let arguments = command.get_args().map(OsStr::to_os_string).collect::<Vec<_>>();
        assert!(arguments.windows(2).any(| v | v == ["--quality", "high"]));
        assert!(arguments.windows(2).any(| v | v == ["--device", "cpu"]));
        assert!(arguments.windows(2).any(| v | v == ["--affinity", "0"]));
        assert!(arguments.iter().any(| v | v == "--threads"));
        assert!(
            arguments.windows(2).any(| v | { v[0] == "--output" && v[1] == directory
            .join("denoised.pfm").as_os_str() })
        );
        assert!(! arguments.iter().any(| v | v == "--out"));
        assert!(
            arguments.iter().any(| v | v == directory.join("beauty.pfm").as_os_str())
        );
        assert!(! arguments.iter().any(| v | v == "--clean_aux"));
        let gpu = oidn_command(executable, directory, OsStr::new("sycl"));
        assert!(! gpu.get_args().any(| v | v == "--threads" || v == "--affinity"));
    }
    #[test]
    fn stop_waits_only_after_successful_termination_and_otherwise_polls() {
        let denied = || std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        let mut calls = Vec::new();
        let result = finish_stop(
            Err(denied()),
            &mut calls,
            |calls| {
                calls.push("wait");
                Ok(0_u8)
            },
            |calls| {
                calls.push("poll");
                Ok(None)
            },
        );
        assert!(result.is_err());
        assert_eq!(calls, ["poll"]);
        calls.clear();
        assert_eq!(
            finish_stop(Err(denied()), & mut calls, | _ |
            panic!("failed kill must not block"), | calls | { calls.push("poll");
            Ok(Some(23_u8)) },).unwrap(), 23
        );
        assert_eq!(calls, ["poll"]);
        calls.clear();
        assert_eq!(
            finish_stop(Ok(()), & mut calls, | calls | { calls.push("wait"); Ok(0_u8) },
            | _ | panic!("successful kill must be reaped"),).unwrap(), 0
        );
        assert_eq!(calls, ["wait"]);
    }
    #[test]
    fn shared_runtime_lookup_is_limited_to_known_launcher_layouts() {
        let base = Path::new("root");
        assert_eq!(
            runtime_roots(& base.join("instances/native-mods")), [base
            .join("instances/native-mods"), base.to_owned()]
        );
        assert_eq!(
            runtime_roots(& base.join("ports/test/home")), [base.join("ports/test/home"),
            base.to_owned()]
        );
        assert_eq!(
            runtime_roots(& base.join("horizon-packs/test/home")), [base
            .join("horizon-packs/test/home"), base.to_owned()]
        );
        assert_eq!(
            runtime_roots(& base.join("custom/nested/home")), [base
            .join("custom/nested/home")]
        );
    }
    #[test]
    #[ignore = "requires an explicit HORIZON_TEST_OIDN_BIN official executable; CPU only"]
    fn real_oidn_cpu_smoke() {
        let executable = official_executable(
                PathBuf::from(
                    std::env::var_os("HORIZON_TEST_OIDN_BIN")
                        .expect("set HORIZON_TEST_OIDN_BIN"),
                ),
            )
            .unwrap();
        let directory = PrivateDirectory::new().unwrap();
        let path = directory.0.join("smoke.png");
        let mut pixels = capture(path.clone());
        pixels.source_size = [32, 32];
        pixels.target_size = [32, 32];
        pixels.beauty = (0..1024)
            .map(|index| [0.3 + (index % 7) as f32 * 0.03; 3])
            .collect();
        pixels.albedo = vec![[0.5; 3]; 1024];
        pixels.normal = vec![[0.0, 1.0, 0.0]; 1024];
        let mut succeeded = false;
        write_using(
                pixels,
                |pixels| {
                    let result = denoise_with(pixels, &executable, OsStr::new("cpu"))?;
                    assert_eq!(result.len(), 1024);
                    assert!(result.iter().flatten().all(| value | value.is_finite()));
                    succeeded = true;
                    Ok(result)
                },
            )
            .unwrap();
        assert!(succeeded, "OIDN smoke used raw fallback");
        assert_eq!(read_png(& path).0, [32, 32]);
        for file in fs::read_dir(&directory.0).unwrap() {
            fs::remove_file(file.unwrap().path()).unwrap();
        }
    }
}

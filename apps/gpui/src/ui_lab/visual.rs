//! PNG decoding, deterministic comparison, and visual diff reports for the UI lab.

use std::{
    fs::{self, File},
    io::{BufReader, ErrorKind, Read, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use image::{ColorType, ImageDecoder, ImageEncoder, Limits};

use super::matrix::validate_generation_manifest;
use super::publication::{publish_file, remove_file_if_present};
use serde::Serialize;

use super::matrix::built_in_matrix;
use super::{MAX_UI_LAB_AREA, MAX_UI_LAB_HEIGHT, MAX_UI_LAB_WIDTH};

const REPORT_SCHEMA_VERSION: u32 = 1;
const MAX_PNG_FILE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PNG_DECODER_ALLOC_BYTES: u64 = MAX_UI_LAB_AREA * 8;

/// Inputs controlling a matrix comparison.
#[derive(Clone, Debug)]
pub struct CompareOptions {
    /// Candidate PNG directory.
    pub actual_dir: PathBuf,
    /// Approved PNG directory.
    pub baseline_dir: PathBuf,
    /// Directory receiving changed-image visualizations.
    pub diff_dir: PathBuf,
    /// JSON report destination.
    pub report: PathBuf,
    /// Maximum per-channel delta ignored for a pixel.
    pub pixel_threshold: u8,
    /// Maximum changed-pixel percentage accepted for a case.
    pub max_diff_percent: f64,
}

/// A deterministic comparison report.
#[derive(Clone, Debug, Serialize)]
pub struct ComparisonReport {
    pub schema_version: u32,
    pub kind: &'static str,
    pub pixel_threshold: u8,
    pub max_diff_percent: f64,
    pub cases: Vec<ComparisonCaseReport>,
}

/// One ordered matrix comparison result.
#[derive(Clone, Debug, Serialize)]
pub struct ComparisonCaseReport {
    pub filename: String,
    pub status: &'static str,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub actual_width: Option<u32>,
    pub actual_height: Option<u32>,
    pub baseline_width: Option<u32>,
    pub baseline_height: Option<u32>,
    pub changed_pixels: Option<u64>,
    pub total_pixels: Option<u64>,
    pub changed_percent: Option<f64>,
    pub maximum_channel_delta: Option<u8>,
    pub diff_filename: Option<String>,
}

/// The result includes whether the CLI should return a nonzero status.
#[derive(Clone, Debug)]
pub struct ComparisonOutcome {
    pub report: ComparisonReport,
    pub has_failures: bool,
}

#[derive(Clone, Copy)]
struct ManifestDimensions {
    actual: (u32, u32),
    baseline: (u32, u32),
}

/// A bounded RGBA image decoded from PNG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DecodedPng {
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) pixels: Vec<u8>,
}

/// Decode a PNG as 8-bit RGBA, checking dimensions before allocating pixel buffers.
pub(crate) fn decode_png(path: &Path) -> Result<DecodedPng> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read PNG metadata {}", path.display()))?;
    if metadata.len() > MAX_PNG_FILE_BYTES {
        bail!("PNG file exceeds the {MAX_PNG_FILE_BYTES}-byte safety limit");
    }
    if !metadata.is_file() {
        bail!("PNG path is not a regular file");
    }
    let file = File::open(path).with_context(|| format!("open PNG {}", path.display()))?;
    let reader = BufReader::new(file.take(MAX_PNG_FILE_BYTES + 1));
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_UI_LAB_WIDTH);
    limits.max_image_height = Some(MAX_UI_LAB_HEIGHT);
    limits.max_alloc = Some(MAX_PNG_DECODER_ALLOC_BYTES);
    let decoder = image::codecs::png::PngDecoder::with_limits(reader, limits)
        .with_context(|| format!("decode PNG header {}", path.display()))?;
    let (width, height) = decoder.dimensions();
    let area = validate_image_dimensions(width, height)?;
    let color_type = decoder.color_type();
    let raw_bytes = decoder.total_bytes();
    let raw_len =
        usize::try_from(raw_bytes).context("PNG decoded byte count does not fit usize")?;
    let max_raw_len = area
        .checked_mul(u64::from(color_type.bytes_per_pixel()))
        .context("PNG decoded byte count overflow")?;
    if raw_bytes > max_raw_len {
        bail!("PNG decoded byte count exceeds the UI-lab safety limit");
    }
    let mut raw = vec![0_u8; raw_len];
    decoder.read_image(&mut raw).context("decode PNG pixels")?;
    let rgba_len = area.checked_mul(4).context("RGBA byte count overflow")?;
    let rgba_len = usize::try_from(rgba_len).context("RGBA byte count does not fit usize")?;
    let pixels = rgba_from_raw(color_type, &raw, rgba_len)?;
    Ok(DecodedPng {
        width,
        height,
        pixels,
    })
}

/// Compare the known matrix and atomically write its report. Missing/malformed inputs are report
/// statuses rather than early errors, so one invocation always gives a complete diagnostic.
pub fn compare_matrix(options: &CompareOptions) -> Result<ComparisonOutcome> {
    validate_compare_options(options)?;
    validate_output_paths(options)?;
    for matrix_case in built_in_matrix() {
        let diff_filename = format!("{}-diff.png", matrix_case.filename.trim_end_matches(".png"));
        remove_file_if_present(&options.diff_dir.join(diff_filename))?;
    }
    let actual_manifest = validate_generation_manifest(&options.actual_dir, "actual")?;
    let baseline_manifest = validate_generation_manifest(&options.baseline_dir, "baseline")?;

    let mut cases = Vec::with_capacity(built_in_matrix().len());
    let mut has_failures = false;
    for ((matrix_case, actual_entry), baseline_entry) in built_in_matrix()
        .iter()
        .zip(actual_manifest.cases.iter())
        .zip(baseline_manifest.cases.iter())
    {
        cases.push(compare_case(
            matrix_case.filename,
            ManifestDimensions {
                actual: (actual_entry.width, actual_entry.height),
                baseline: (baseline_entry.width, baseline_entry.height),
            },
            options,
            &mut has_failures,
        )?);
    }

    let report = ComparisonReport {
        schema_version: REPORT_SCHEMA_VERSION,
        kind: "jira-ui-lab-comparison",
        pixel_threshold: options.pixel_threshold,
        max_diff_percent: options.max_diff_percent,
        cases,
    };
    publish_json(&options.report, &report).context("publish comparison report")?;
    Ok(ComparisonOutcome {
        report,
        has_failures,
    })
}

fn validate_compare_options(options: &CompareOptions) -> Result<()> {
    if !options.max_diff_percent.is_finite() || !(0.0..=100.0).contains(&options.max_diff_percent) {
        bail!("--max-diff-percent must be a finite number from 0 to 100");
    }
    Ok(())
}

fn compare_case(
    filename: &str,
    expected: ManifestDimensions,
    options: &CompareOptions,
    has_failures: &mut bool,
) -> Result<ComparisonCaseReport> {
    let actual = decode_if_present(&options.actual_dir.join(filename));
    let baseline = decode_if_present(&options.baseline_dir.join(filename));
    compare_case_inputs(filename, actual, baseline, expected, options, has_failures)
}

fn compare_case_inputs(
    filename: &str,
    actual: Option<Result<DecodedPng>>,
    baseline: Option<Result<DecodedPng>>,
    expected: ManifestDimensions,
    options: &CompareOptions,
    has_failures: &mut bool,
) -> Result<ComparisonCaseReport> {
    match (actual, baseline) {
        (None, None) => failed_missing(filename, "missing-actual-and-baseline", has_failures),
        (None, Some(_)) => failed_missing(filename, "missing-actual", has_failures),
        (Some(_), None) => failed_missing(filename, "missing-baseline", has_failures),
        (Some(Err(_)), Some(Err(_))) => {
            failed_missing(filename, "malformed-actual-and-baseline", has_failures)
        }
        (Some(Err(_)), _) => failed_missing(filename, "malformed-actual", has_failures),
        (_, Some(Err(_))) => failed_missing(filename, "malformed-baseline", has_failures),
        (Some(Ok(actual)), Some(Ok(baseline))) => {
            if actual.width != baseline.width || actual.height != baseline.height {
                return compare_images(filename, actual, baseline, options, has_failures);
            }
            if (actual.width, actual.height) != expected.actual
                || (baseline.width, baseline.height) != expected.baseline
            {
                *has_failures = true;
                return Ok(manifest_dimension_mismatch_report(
                    filename, actual, baseline,
                ));
            }
            compare_images(filename, actual, baseline, options, has_failures)
        }
    }
}

fn failed_missing(
    filename: &str,
    status: &'static str,
    has_failures: &mut bool,
) -> Result<ComparisonCaseReport> {
    *has_failures = true;
    Ok(missing_report(filename, status))
}

fn decode_if_present(path: &Path) -> Option<Result<DecodedPng>> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => Some(decode_png(path)),
        Ok(_) => Some(Err(anyhow::anyhow!("PNG path is not a regular file"))),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => Some(Err(error.into())),
    }
}

fn validate_output_paths(options: &CompareOptions) -> Result<()> {
    let protected = [
        ("baseline", resolved_path(&options.baseline_dir)?),
        ("actual", resolved_path(&options.actual_dir)?),
    ];
    for (output_name, output) in [("diff", &options.diff_dir), ("report", &options.report)] {
        let resolved_output = resolved_path(output)?;
        for (protected_name, protected_path) in &protected {
            if resolved_output.starts_with(protected_path) {
                bail!(
                    "{output_name} destination {} must not equal or be inside {protected_name} directory {}",
                    output.display(),
                    protected_path.display()
                );
            }
        }
    }
    Ok(())
}

fn resolved_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .context("resolve current directory for output path")?
            .join(path)
    };
    let mut probe = absolute;
    let mut missing = Vec::new();
    loop {
        match fs::canonicalize(&probe) {
            Ok(existing) => {
                let mut resolved = existing;
                for component in missing.iter().rev() {
                    resolved.push(component);
                }
                return Ok(lexically_normalize(&resolved));
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                let name = probe.file_name().ok_or_else(|| {
                    anyhow::anyhow!("cannot resolve output path {}", path.display())
                })?;
                missing.push(name.to_owned());
                if !probe.pop() {
                    bail!("cannot resolve output path {}", path.display());
                }
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("resolve output path {}", path.display()));
            }
        }
    }
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            Component::Normal(name) => normalized.push(name),
        }
    }
    normalized
}

fn compare_images(
    filename: &str,
    actual: DecodedPng,
    baseline: DecodedPng,
    options: &CompareOptions,
    has_failures: &mut bool,
) -> Result<ComparisonCaseReport> {
    if actual.width != baseline.width || actual.height != baseline.height {
        *has_failures = true;
        return Ok(ComparisonCaseReport {
            filename: filename.to_owned(),
            status: "dimension-mismatch",
            width: None,
            height: None,
            actual_width: Some(actual.width),
            actual_height: Some(actual.height),
            baseline_width: Some(baseline.width),
            baseline_height: Some(baseline.height),
            changed_pixels: None,
            total_pixels: None,
            changed_percent: None,
            maximum_channel_delta: None,
            diff_filename: None,
        });
    }

    let total_pixels = validate_image_dimensions(actual.width, actual.height)?;
    let expected_len = usize::try_from(total_pixels.checked_mul(4).context("pixel byte overflow")?)
        .context("pixel byte count does not fit usize")?;
    if actual.pixels.len() != expected_len || baseline.pixels.len() != expected_len {
        bail!("decoded PNG pixel buffer has an unexpected size");
    }
    let mut changed_pixels = 0_u64;
    let mut maximum_channel_delta = 0_u8;
    let mut diff_pixels = Vec::with_capacity(expected_len);
    for (actual_pixel, baseline_pixel) in actual
        .pixels
        .chunks_exact(4)
        .zip(baseline.pixels.chunks_exact(4))
    {
        let channel_delta = actual_pixel
            .iter()
            .zip(baseline_pixel.iter())
            .map(|(actual, baseline)| actual.abs_diff(*baseline))
            .max()
            .unwrap_or(0);
        maximum_channel_delta = maximum_channel_delta.max(channel_delta);
        if channel_delta > options.pixel_threshold {
            changed_pixels = changed_pixels
                .checked_add(1)
                .context("changed pixel count overflow")?;
            diff_pixels.extend_from_slice(&[255, 0, 180, 255]);
        } else {
            let luminance = (u16::from(actual_pixel[0]) * 30
                + u16::from(actual_pixel[1]) * 59
                + u16::from(actual_pixel[2]) * 11)
                / 100
                / 2;
            let luminance = u8::try_from(luminance).unwrap_or(u8::MAX);
            diff_pixels.extend_from_slice(&[luminance, luminance, luminance, 255]);
        }
    }
    let changed_percent = (changed_pixels as f64 * 100.0) / total_pixels as f64;
    let within_tolerance = changed_percent <= options.max_diff_percent;
    if !within_tolerance {
        *has_failures = true;
    }
    let diff_filename = if changed_pixels > 0 {
        let diff_filename = format!("{}-diff.png", filename.trim_end_matches(".png"));
        write_diff(
            &options.diff_dir,
            &diff_filename,
            &diff_pixels,
            actual.width,
            actual.height,
        )?;
        Some(diff_filename)
    } else {
        None
    };
    Ok(ComparisonCaseReport {
        filename: filename.to_owned(),
        status: if changed_pixels == 0 {
            "matched"
        } else if within_tolerance {
            "within-tolerance"
        } else {
            "exceeds-tolerance"
        },
        width: Some(actual.width),
        height: Some(actual.height),
        actual_width: Some(actual.width),
        actual_height: Some(actual.height),
        baseline_width: Some(baseline.width),
        baseline_height: Some(baseline.height),
        changed_pixels: Some(changed_pixels),
        total_pixels: Some(total_pixels),
        changed_percent: Some(changed_percent),
        maximum_channel_delta: Some(maximum_channel_delta),
        diff_filename,
    })
}

fn missing_report(filename: &str, status: &'static str) -> ComparisonCaseReport {
    ComparisonCaseReport {
        filename: filename.to_owned(),
        status,
        width: None,
        height: None,
        actual_width: None,
        actual_height: None,
        baseline_width: None,
        baseline_height: None,
        changed_pixels: None,
        total_pixels: None,
        changed_percent: None,
        maximum_channel_delta: None,
        diff_filename: None,
    }
}

fn manifest_dimension_mismatch_report(
    filename: &str,
    actual: DecodedPng,
    baseline: DecodedPng,
) -> ComparisonCaseReport {
    ComparisonCaseReport {
        filename: filename.to_owned(),
        status: "manifest-dimension-mismatch",
        width: None,
        height: None,
        actual_width: Some(actual.width),
        actual_height: Some(actual.height),
        baseline_width: Some(baseline.width),
        baseline_height: Some(baseline.height),
        changed_pixels: None,
        total_pixels: None,
        changed_percent: None,
        maximum_channel_delta: None,
        diff_filename: None,
    }
}

pub(crate) fn validate_image_dimensions(width: u32, height: u32) -> Result<u64> {
    if width == 0 || height == 0 {
        bail!("PNG dimensions must be nonzero");
    }
    if width > MAX_UI_LAB_WIDTH || height > MAX_UI_LAB_HEIGHT {
        bail!("PNG dimensions exceed the UI-lab limit of {MAX_UI_LAB_WIDTH}x{MAX_UI_LAB_HEIGHT}");
    }
    u64::from(width)
        .checked_mul(u64::from(height))
        .filter(|area| *area <= MAX_UI_LAB_AREA)
        .context("PNG pixel area overflow or exceeds UI-lab limit")
}

fn rgba_from_raw(color_type: ColorType, raw: &[u8], rgba_len: usize) -> Result<Vec<u8>> {
    let channels = usize::from(color_type.bytes_per_pixel());
    let pixel_count = raw
        .len()
        .checked_div(channels)
        .filter(|_| raw.len().is_multiple_of(channels))
        .context("PNG decoder returned an inconsistent pixel buffer")?;
    if pixel_count.checked_mul(4) != Some(rgba_len) {
        bail!("PNG decoder returned an inconsistent pixel buffer");
    }
    let mut rgba = Vec::with_capacity(rgba_len);
    match color_type {
        ColorType::L8 => {
            for pixel in raw {
                rgba.extend_from_slice(&[*pixel, *pixel, *pixel, 255]);
            }
        }
        ColorType::La8 => {
            for pixel in raw.chunks_exact(2) {
                rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]);
            }
        }
        ColorType::Rgb8 => {
            for pixel in raw.chunks_exact(3) {
                rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]);
            }
        }
        ColorType::Rgba8 => rgba.extend_from_slice(raw),
        ColorType::L16 => {
            for pixel in raw.chunks_exact(2) {
                let value = u16::from_ne_bytes([pixel[0], pixel[1]]) / 257;
                let value = u8::try_from(value).unwrap_or(u8::MAX);
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        ColorType::La16 => {
            for pixel in raw.chunks_exact(4) {
                let value =
                    u8::try_from(u16::from_ne_bytes([pixel[0], pixel[1]]) / 257).unwrap_or(u8::MAX);
                let alpha =
                    u8::try_from(u16::from_ne_bytes([pixel[2], pixel[3]]) / 257).unwrap_or(u8::MAX);
                rgba.extend_from_slice(&[value, value, value, alpha]);
            }
        }
        ColorType::Rgb16 => {
            for pixel in raw.chunks_exact(6) {
                rgba.extend_from_slice(&[
                    u8::try_from(u16::from_ne_bytes([pixel[0], pixel[1]]) / 257).unwrap_or(u8::MAX),
                    u8::try_from(u16::from_ne_bytes([pixel[2], pixel[3]]) / 257).unwrap_or(u8::MAX),
                    u8::try_from(u16::from_ne_bytes([pixel[4], pixel[5]]) / 257).unwrap_or(u8::MAX),
                    255,
                ]);
            }
        }
        ColorType::Rgba16 => {
            for pixel in raw.chunks_exact(8) {
                rgba.extend_from_slice(&[
                    u8::try_from(u16::from_ne_bytes([pixel[0], pixel[1]]) / 257).unwrap_or(u8::MAX),
                    u8::try_from(u16::from_ne_bytes([pixel[2], pixel[3]]) / 257).unwrap_or(u8::MAX),
                    u8::try_from(u16::from_ne_bytes([pixel[4], pixel[5]]) / 257).unwrap_or(u8::MAX),
                    u8::try_from(u16::from_ne_bytes([pixel[6], pixel[7]]) / 257).unwrap_or(u8::MAX),
                ]);
            }
        }
        _ => bail!("PNG color type is not supported by the UI-lab comparator"),
    }
    if rgba.len() != rgba_len {
        bail!("PNG RGBA conversion returned an inconsistent buffer");
    }
    Ok(rgba)
}

fn write_diff(
    diff_dir: &Path,
    filename: &str,
    pixels: &[u8],
    width: u32,
    height: u32,
) -> Result<()> {
    fs::create_dir_all(diff_dir)
        .with_context(|| format!("create diff directory {}", diff_dir.display()))?;
    publish_file(&diff_dir.join(filename), "diff", |file| {
        image::codecs::png::PngEncoder::new(file)
            .write_image(pixels, width, height, image::ExtendedColorType::Rgba8)
            .context("encode visual diff PNG")
    })
}

fn publish_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).context("serialize comparison report")?;
    bytes.push(b'\n');
    publish_file(path, "report", |file| {
        file.write_all(&bytes)
            .context("write comparison report bytes")
    })
}

#[cfg(test)]
mod tests {
    use super::{CompareOptions, compare_matrix, decode_png, validate_image_dimensions};
    use crate::ui_lab::matrix::{
        MATRIX_MANIFEST_FILENAME, MATRIX_SCHEMA_VERSION, MatrixManifest, manifest_case,
    };
    use image::{ExtendedColorType, ImageEncoder};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    fn png(path: &Path, width: u32, height: u32, pixels: &[u8]) {
        let file = fs::File::create(path).unwrap();
        image::codecs::png::PngEncoder::new(file)
            .write_image(pixels, width, height, ExtendedColorType::Rgba8)
            .unwrap();
    }

    fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let root =
            std::env::temp_dir().join(format!("jira-ui-visual-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let actual = root.join("actual");
        let baseline = root.join("baseline");
        fs::create_dir_all(&actual).unwrap();
        fs::create_dir_all(&baseline).unwrap();
        (
            actual,
            baseline,
            root.join("diff"),
            root.join("report.json"),
        )
    }

    fn populate(actual: &Path, baseline: &Path, first_actual: &[u8], first_baseline: &[u8]) {
        for (index, case) in super::built_in_matrix().iter().enumerate() {
            let (actual_pixels, baseline_pixels) = if index == 0 {
                (first_actual, first_baseline)
            } else {
                (
                    &[0, 0, 0, 255, 0, 0, 0, 255][..],
                    &[0, 0, 0, 255, 0, 0, 0, 255][..],
                )
            };
            png(&actual.join(case.filename), 2, 1, actual_pixels);
            png(&baseline.join(case.filename), 2, 1, baseline_pixels);
        }
        let manifest = MatrixManifest {
            schema_version: MATRIX_SCHEMA_VERSION,
            kind: "jira-ui-lab-matrix".to_owned(),
            cases: super::built_in_matrix()
                .iter()
                .map(|case| {
                    manifest_case(
                        *case,
                        crate::ui_lab::UiLabCaptureReport {
                            width: 2,
                            height: 1,
                        },
                    )
                })
                .collect(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        fs::write(actual.join(MATRIX_MANIFEST_FILENAME), &manifest_bytes).unwrap();
        fs::write(baseline.join(MATRIX_MANIFEST_FILENAME), manifest_bytes).unwrap();
    }

    fn options(
        actual: PathBuf,
        baseline: PathBuf,
        diff: PathBuf,
        report: PathBuf,
        threshold: u8,
        percent: f64,
    ) -> CompareOptions {
        CompareOptions {
            actual_dir: actual,
            baseline_dir: baseline,
            diff_dir: diff,
            report,
            pixel_threshold: threshold,
            max_diff_percent: percent,
        }
    }

    #[test]
    fn exact_match_and_channel_threshold_are_strict_and_inclusive() {
        let (actual, baseline, diff, report) = setup("threshold");
        populate(
            &actual,
            &baseline,
            &[10, 20, 30, 255, 100, 100, 100, 255],
            &[10, 20, 31, 255, 100, 100, 100, 255],
        );
        let outcome = compare_matrix(&options(
            actual.clone(),
            baseline.clone(),
            diff.clone(),
            report.clone(),
            1,
            0.0,
        ))
        .unwrap();
        assert!(!outcome.has_failures);
        assert_eq!(outcome.report.cases[0].changed_pixels, Some(0));
        assert_eq!(outcome.report.cases[0].maximum_channel_delta, Some(1));
        assert!(!diff.exists());

        let outcome =
            compare_matrix(&options(actual, baseline, diff.clone(), report, 0, 0.0)).unwrap();
        assert!(outcome.has_failures);
        assert_eq!(outcome.report.cases[0].changed_pixels, Some(1));
        assert_eq!(outcome.report.cases[0].total_pixels, Some(2));
        assert_eq!(outcome.report.cases[0].status, "exceeds-tolerance");
        assert_eq!(
            outcome.report.cases[0].diff_filename.as_deref(),
            Some("onboarding-light-960x700-diff.png")
        );
        assert_eq!(
            decode_png(&diff.join("onboarding-light-960x700-diff.png"))
                .unwrap()
                .pixels,
            vec![255, 0, 180, 255, 50, 50, 50, 255]
        );
        let _ = fs::remove_dir_all(diff.parent().unwrap());
    }

    #[test]
    fn changed_then_matched_or_invalid_removes_stale_known_diff() {
        let (actual, baseline, diff, report) = setup("stale-diff");
        populate(
            &actual,
            &baseline,
            &[255, 0, 0, 255, 0, 0, 0, 255],
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        let options = options(
            actual.clone(),
            baseline.clone(),
            diff.clone(),
            report.clone(),
            0,
            0.0,
        );
        assert!(compare_matrix(&options).unwrap().has_failures);
        let known_diff = diff.join("onboarding-light-960x700-diff.png");
        assert!(known_diff.is_file());

        png(
            &actual.join("onboarding-light-960x700.png"),
            2,
            1,
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        assert!(!compare_matrix(&options).unwrap().has_failures);
        assert!(!known_diff.exists());

        png(
            &actual.join("onboarding-light-960x700.png"),
            2,
            1,
            &[255, 0, 0, 255, 0, 0, 0, 255],
        );
        assert!(compare_matrix(&options).unwrap().has_failures);
        assert!(known_diff.is_file());
        fs::write(actual.join("onboarding-light-960x700.png"), b"malformed").unwrap();
        assert!(compare_matrix(&options).unwrap().has_failures);
        assert!(!known_diff.exists());
        let _ = fs::remove_dir_all(actual.parent().unwrap());
    }

    #[test]
    fn percentage_boundary_and_report_order_are_deterministic() {
        let (actual, baseline, diff, report) = setup("percentage");
        populate(
            &actual,
            &baseline,
            &[2, 0, 0, 255, 0, 0, 0, 255],
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        let outcome = compare_matrix(&options(
            actual.clone(),
            baseline.clone(),
            diff,
            report.clone(),
            0,
            50.0,
        ))
        .unwrap();
        assert!(!outcome.has_failures);
        assert_eq!(outcome.report.cases[0].changed_percent, Some(50.0));
        // One of two pixels differs, so 50% is accepted at the exact inclusive boundary.
        let first_report_bytes = fs::read(&report).unwrap();
        let outcome = compare_matrix(&options(
            actual.clone(),
            baseline,
            setup("unused").2,
            report.clone(),
            0,
            50.0,
        ))
        .unwrap();
        assert!(!outcome.has_failures);
        assert_eq!(fs::read(report).unwrap(), first_report_bytes);
        assert_eq!(
            outcome
                .report
                .cases
                .iter()
                .map(|case| case.filename.as_str())
                .collect::<Vec<_>>(),
            vec![
                "onboarding-light-960x700.png",
                "issues-dark-1280x900.png",
                "updates-light-1095x700.png",
                "team-dark-1370x900.png",
                "settings-light-960x700.png"
            ]
        );
        let _ = fs::remove_dir_all(actual.parent().unwrap());
    }

    #[test]
    fn missing_malformed_and_dimension_mismatch_have_no_diff_artifact() {
        let (actual, baseline, diff, report) = setup("invalid");
        populate(
            &actual,
            &baseline,
            &[0, 0, 0, 255, 0, 0, 0, 255],
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        fs::remove_file(actual.join("issues-dark-1280x900.png")).unwrap();
        png(
            &baseline.join("updates-light-1095x700.png"),
            1,
            1,
            &[0, 0, 0, 255],
        );
        fs::write(actual.join("team-dark-1370x900.png"), b"not a png").unwrap();
        fs::remove_file(baseline.join("settings-light-960x700.png")).unwrap();
        let outcome = compare_matrix(&options(
            actual.clone(),
            baseline,
            diff.clone(),
            report,
            0,
            0.0,
        ))
        .unwrap();
        assert!(outcome.has_failures);
        assert_eq!(outcome.report.cases[1].status, "missing-actual");
        assert_eq!(outcome.report.cases[2].status, "dimension-mismatch");
        assert_eq!(outcome.report.cases[3].status, "malformed-actual");
        assert_eq!(outcome.report.cases[4].status, "missing-baseline");
        assert!(!diff.join("issues-dark-1280x900-diff.png").exists());
        assert!(!diff.join("updates-light-1095x700-diff.png").exists());
        assert!(!diff.join("team-dark-1370x900-diff.png").exists());
        let _ = fs::remove_dir_all(actual.parent().unwrap());
    }

    #[test]
    fn missing_generation_manifest_fails_before_report_publication() {
        let (actual, baseline, diff, report) = setup("missing-manifest");
        populate(
            &actual,
            &baseline,
            &[0, 0, 0, 255, 0, 0, 0, 255],
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        fs::create_dir_all(&diff).unwrap();
        let known_diff_paths = super::built_in_matrix()
            .iter()
            .map(|case| {
                diff.join(format!(
                    "{}-diff.png",
                    case.filename.trim_end_matches(".png")
                ))
            })
            .collect::<Vec<_>>();
        for path in &known_diff_paths {
            fs::write(path, b"stale diff").unwrap();
        }
        let unrelated_marker = diff.join("unrelated-marker.txt");
        fs::write(&unrelated_marker, b"preserve me").unwrap();
        fs::remove_file(actual.join(MATRIX_MANIFEST_FILENAME)).unwrap();
        assert!(compare_matrix(&options(actual, baseline, diff, report.clone(), 0, 0.0)).is_err());
        assert!(!report.exists());
        for path in known_diff_paths {
            assert!(!path.exists());
        }
        assert_eq!(fs::read(&unrelated_marker).unwrap(), b"preserve me");
    }

    #[cfg(unix)]
    #[test]
    fn output_paths_cannot_alias_baseline_or_actual_tree() {
        use std::os::unix::fs::symlink;

        let (actual, baseline, diff, _report) = setup("path-protection");
        populate(
            &actual,
            &baseline,
            &[0, 0, 0, 255, 0, 0, 0, 255],
            &[0, 0, 0, 255, 0, 0, 0, 255],
        );
        let root = actual.parent().unwrap().to_owned();
        for (diff_path, report_path) in [
            (baseline.clone(), root.join("safe-report.json")),
            (
                root.join("baseline/../baseline"),
                root.join("safe-report.json"),
            ),
            (root.join("safe-diff"), baseline.join("report.json")),
        ] {
            let error = compare_matrix(&options(
                actual.clone(),
                baseline.clone(),
                diff_path,
                report_path,
                0,
                0.0,
            ))
            .unwrap_err();
            assert!(error.to_string().contains("must not equal or be inside"));
        }

        let baseline_link = root.join("baseline-link");
        symlink(&baseline, &baseline_link).unwrap();
        let error = compare_matrix(&options(
            actual.clone(),
            baseline.clone(),
            baseline_link.join("diff"),
            root.join("safe-report-2.json"),
            0,
            0.0,
        ))
        .unwrap_err();
        assert!(error.to_string().contains("must not equal or be inside"));
        assert!(!root.join("safe-report.json").exists());
        assert!(!diff.exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn oversized_ihdr_is_rejected_before_decoder_allocation() {
        let root =
            std::env::temp_dir().join(format!("jira-ui-oversized-png-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let path = root.join("oversized.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&(super::MAX_UI_LAB_WIDTH + 1).to_be_bytes());
        ihdr.extend_from_slice(&1_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        bytes.extend_from_slice(&13_u32.to_be_bytes());
        bytes.extend_from_slice(b"IHDR");
        bytes.extend_from_slice(&ihdr);
        bytes.extend_from_slice(&crc32(b"IHDR", &ihdr).to_be_bytes());
        fs::write(&path, bytes).unwrap();
        assert!(decode_png(&path).is_err());
        let _ = fs::remove_dir_all(root);
    }

    fn crc32(kind: &[u8], data: &[u8]) -> u32 {
        let mut crc = u32::MAX;
        for byte in kind.iter().chain(data) {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn decoded_dimensions_are_bounded_before_pixel_allocation() {
        assert!(validate_image_dimensions(0, 1).is_err());
        assert!(validate_image_dimensions(super::MAX_UI_LAB_WIDTH + 1, 1).is_err());
        assert!(validate_image_dimensions(1, super::MAX_UI_LAB_HEIGHT + 1).is_err());
        assert!(validate_image_dimensions(4096, 2160).is_err());
        assert!(validate_image_dimensions(320, 240).is_ok());
    }
}

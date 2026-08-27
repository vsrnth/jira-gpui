//! The stable capture matrix and its candidate/acceptance filesystem operations.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use super::publication::{publish_file, remove_file_if_present};
use super::visual::decode_png;
use super::{UiLabCapture, UiLabCaptureReport, UiLabScenario, UiLabSize, UiLabTheme, capture};

/// The filename written alongside a candidate matrix.
pub const MATRIX_MANIFEST_FILENAME: &str = "matrix-manifest.json";
pub(crate) const MATRIX_SCHEMA_VERSION: u32 = 1;
const MATRIX_KIND: &str = "jira-ui-lab-matrix";

/// One stable, semantic capture matrix entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UiLabMatrixCase {
    /// Fixture scenario.
    pub scenario: UiLabScenario,
    /// Component theme.
    pub theme: UiLabTheme,
    /// Logical capture size.
    pub size: UiLabSize,
    /// Stable PNG filename.
    pub filename: &'static str,
}

impl UiLabMatrixCase {
    fn request(self, output_dir: &Path) -> UiLabCapture {
        UiLabCapture {
            scenario: self.scenario,
            theme: self.theme,
            size: self.size,
            output: output_dir.join(self.filename),
        }
    }
}

/// The one explicit, ordered built-in matrix. Do not derive this from directory contents.
pub const fn built_in_matrix() -> &'static [UiLabMatrixCase] {
    &[
        UiLabMatrixCase {
            scenario: UiLabScenario::Onboarding,
            theme: UiLabTheme::Light,
            size: UiLabSize {
                width: 960,
                height: 700,
            },
            filename: "onboarding-light-960x700.png",
        },
        UiLabMatrixCase {
            scenario: UiLabScenario::Issues,
            theme: UiLabTheme::Dark,
            size: UiLabSize {
                width: 1280,
                height: 900,
            },
            filename: "issues-dark-1280x900.png",
        },
        UiLabMatrixCase {
            scenario: UiLabScenario::Updates,
            theme: UiLabTheme::Light,
            size: UiLabSize {
                width: 1095,
                height: 700,
            },
            filename: "updates-light-1095x700.png",
        },
        UiLabMatrixCase {
            scenario: UiLabScenario::Team,
            theme: UiLabTheme::Dark,
            size: UiLabSize {
                width: 1370,
                height: 900,
            },
            filename: "team-dark-1370x900.png",
        },
        UiLabMatrixCase {
            scenario: UiLabScenario::Settings,
            theme: UiLabTheme::Light,
            size: UiLabSize {
                width: 960,
                height: 700,
            },
            filename: "settings-light-960x700.png",
        },
    ]
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MatrixManifestCase {
    pub(crate) filename: String,
    pub(crate) scenario: String,
    pub(crate) theme: String,
    pub(crate) logical_width: u32,
    pub(crate) logical_height: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MatrixManifest {
    pub(crate) schema_version: u32,
    pub(crate) kind: String,
    pub(crate) cases: Vec<MatrixManifestCase>,
}

/// Capture all matrix entries sequentially and publish the manifest last.
pub fn capture_matrix(output_dir: &Path) -> Result<()> {
    fs::create_dir_all(output_dir)
        .with_context(|| format!("create matrix output directory {}", output_dir.display()))?;
    invalidate_manifest(output_dir)?;

    let mut manifest_cases = Vec::with_capacity(built_in_matrix().len());
    for case in built_in_matrix() {
        let request = case.request(output_dir);
        let report =
            capture(&request).with_context(|| format!("capture matrix case {}", case.filename))?;
        manifest_cases.push(manifest_case(*case, report));
    }

    let manifest = MatrixManifest {
        schema_version: MATRIX_SCHEMA_VERSION,
        kind: MATRIX_KIND.to_owned(),
        cases: manifest_cases,
    };
    publish_json(&output_dir.join(MATRIX_MANIFEST_FILENAME), &manifest)
}

/// Publish a complete, explicitly reviewed candidate matrix into a baseline directory.
pub fn accept_baselines(
    actual_dir: &Path,
    baseline_dir: &Path,
    confirm_reviewed: bool,
) -> Result<()> {
    if !confirm_reviewed {
        bail!("baseline acceptance requires the exact --confirm-reviewed flag");
    }

    let (manifest, candidates) = validate_complete_matrix(actual_dir)?;
    // Invalidate first: any later candidate-copy or manifest-publication failure must not leave a
    // manifest that makes a partial baseline look complete.
    invalidate_manifest(baseline_dir)?;
    fs::create_dir_all(baseline_dir)
        .with_context(|| format!("create baseline directory {}", baseline_dir.display()))?;
    for (filename, source) in candidates {
        publish_copy(&source, &baseline_dir.join(filename))
            .with_context(|| format!("publish baseline {filename}"))?;
    }
    publish_json(&baseline_dir.join(MATRIX_MANIFEST_FILENAME), &manifest)
        .context("publish baseline matrix manifest")
}

fn validate_complete_matrix(
    actual_dir: &Path,
) -> Result<(MatrixManifest, Vec<(&'static str, PathBuf)>)> {
    let manifest =
        read_and_validate_manifest(&actual_dir.join(MATRIX_MANIFEST_FILENAME), "candidate")?;
    let mut candidates = Vec::with_capacity(built_in_matrix().len());
    for (case, entry) in built_in_matrix().iter().zip(manifest.cases.iter()) {
        let path = actual_dir.join(case.filename);
        let decoded =
            decode_png(&path).with_context(|| format!("validate candidate {}", case.filename))?;
        if decoded.width != entry.width || decoded.height != entry.height {
            bail!(
                "candidate {} dimensions do not match matrix manifest ({}x{} vs {}x{})",
                case.filename,
                decoded.width,
                decoded.height,
                entry.width,
                entry.height
            );
        }
        candidates.push((case.filename, path));
    }
    Ok((manifest, candidates))
}

/// Validate generation metadata and dimensions for every decodable known image. Missing or
/// malformed images remain comparison statuses so the report can identify the affected case.
pub(crate) fn validate_generation_manifest(dir: &Path, label: &str) -> Result<MatrixManifest> {
    // Missing and malformed PNGs are deliberately deferred to comparison case statuses. The
    // manifest itself is still mandatory and its declared dimensions are bounded below.
    read_and_validate_manifest(&dir.join(MATRIX_MANIFEST_FILENAME), label)
}

pub(crate) fn manifest_case(
    case: UiLabMatrixCase,
    report: UiLabCaptureReport,
) -> MatrixManifestCase {
    MatrixManifestCase {
        filename: case.filename.to_owned(),
        scenario: case.scenario.as_str().to_owned(),
        theme: case.theme.as_str().to_owned(),
        logical_width: case.size.width,
        logical_height: case.size.height,
        width: report.width,
        height: report.height,
    }
}

pub(crate) fn read_and_validate_manifest(path: &Path, label: &str) -> Result<MatrixManifest> {
    let bytes = fs::read(path)
        .with_context(|| format!("read {label} matrix manifest {}", path.display()))?;
    let manifest: MatrixManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {label} matrix manifest {}", path.display()))?;
    if manifest.schema_version != MATRIX_SCHEMA_VERSION || manifest.kind != MATRIX_KIND {
        bail!("{label} matrix manifest has an unsupported schema or kind");
    }
    if manifest.cases.len() != built_in_matrix().len() {
        bail!("{label} matrix manifest does not contain exactly the built-in matrix");
    }
    for (case, entry) in built_in_matrix().iter().zip(manifest.cases.iter()) {
        if entry.filename != case.filename
            || entry.scenario != case.scenario.as_str()
            || entry.theme != case.theme.as_str()
            || entry.logical_width != case.size.width
            || entry.logical_height != case.size.height
        {
            bail!("{label} matrix manifest has an unexpected entry order or metadata");
        }
        super::visual::validate_image_dimensions(entry.width, entry.height).with_context(|| {
            format!("validate {label} manifest dimensions for {}", case.filename)
        })?;
    }
    Ok(manifest)
}

fn invalidate_manifest(dir: &Path) -> Result<()> {
    remove_file_if_present(&dir.join(MATRIX_MANIFEST_FILENAME))
}

fn publish_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value).context("serialize matrix manifest")?;
    bytes.push(b'\n');
    publish_file(path, "manifest", |file| {
        file.write_all(&bytes).context("write manifest bytes")
    })
}

fn publish_copy(source: &Path, destination: &Path) -> Result<()> {
    let mut source_file =
        File::open(source).with_context(|| format!("open candidate {}", source.display()))?;
    publish_file(destination, "baseline", |temporary| {
        io::copy(&mut source_file, temporary).context("copy candidate bytes")?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::super::UiLabCaptureReport;
    use super::{
        MATRIX_MANIFEST_FILENAME, MATRIX_SCHEMA_VERSION, MatrixManifest, accept_baselines,
        built_in_matrix, invalidate_manifest, manifest_case,
    };
    use image::{ExtendedColorType, ImageEncoder};
    use std::{fs, path::Path};

    #[test]
    fn matrix_is_explicitly_ordered_and_named() {
        assert_eq!(
            built_in_matrix()
                .iter()
                .map(|case| case.filename)
                .collect::<Vec<_>>(),
            vec![
                "onboarding-light-960x700.png",
                "issues-dark-1280x900.png",
                "updates-light-1095x700.png",
                "team-dark-1370x900.png",
                "settings-light-960x700.png",
            ]
        );
        assert_eq!(MATRIX_MANIFEST_FILENAME, "matrix-manifest.json");
    }

    fn png(path: &Path) {
        image::codecs::png::PngEncoder::new(fs::File::create(path).unwrap())
            .write_image(
                &[0, 0, 0, 255, 0, 0, 0, 255],
                2,
                1,
                ExtendedColorType::Rgba8,
            )
            .unwrap();
    }

    fn candidate(name: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "jira-ui-acceptance-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let actual = root.join("actual");
        let baseline = root.join("baseline");
        fs::create_dir_all(&actual).unwrap();
        fs::create_dir_all(&baseline).unwrap();
        for case in built_in_matrix() {
            png(&actual.join(case.filename));
        }
        let manifest = MatrixManifest {
            schema_version: MATRIX_SCHEMA_VERSION,
            kind: "jira-ui-lab-matrix".to_owned(),
            cases: built_in_matrix()
                .iter()
                .map(|case| {
                    manifest_case(
                        *case,
                        UiLabCaptureReport {
                            width: 2,
                            height: 1,
                        },
                    )
                })
                .collect(),
        };
        fs::write(
            actual.join(MATRIX_MANIFEST_FILENAME),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        (root, actual, baseline)
    }

    #[test]
    fn acceptance_requires_confirmation_and_preserves_unrelated_files() {
        let (root, actual, baseline) = candidate("confirmed");
        fs::write(baseline.join("keep.txt"), b"keep").unwrap();
        assert!(accept_baselines(&actual, &baseline, false).is_err());
        assert_eq!(fs::read(baseline.join("keep.txt")).unwrap(), b"keep");
        assert_eq!(fs::read_dir(&baseline).unwrap().count(), 1);
        accept_baselines(&actual, &baseline, true).unwrap();
        for case in built_in_matrix() {
            assert!(baseline.join(case.filename).is_file());
        }
        assert!(baseline.join(MATRIX_MANIFEST_FILENAME).is_file());
        assert_eq!(fs::read(baseline.join("keep.txt")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_candidates_are_rejected_before_baseline_touch() {
        let (root, actual, baseline) = candidate("incomplete");
        fs::write(baseline.join("sentinel.txt"), b"untouched").unwrap();
        fs::remove_file(actual.join("team-dark-1370x900.png")).unwrap();
        assert!(accept_baselines(&actual, &baseline, true).is_err());
        assert_eq!(
            fs::read(baseline.join("sentinel.txt")).unwrap(),
            b"untouched"
        );
        assert_eq!(fs::read_dir(&baseline).unwrap().count(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn failed_baseline_publication_leaves_no_valid_old_manifest() {
        let (root, actual, baseline) = candidate("publication-failure");
        fs::write(baseline.join(MATRIX_MANIFEST_FILENAME), b"old").unwrap();
        fs::create_dir(baseline.join(built_in_matrix()[0].filename)).unwrap();
        assert!(accept_baselines(&actual, &baseline, true).is_err());
        assert!(!baseline.join(MATRIX_MANIFEST_FILENAME).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalidating_manifest_preserves_unrelated_files() {
        let (root, _actual, baseline) = candidate("invalidate");
        fs::write(baseline.join(MATRIX_MANIFEST_FILENAME), b"old").unwrap();
        fs::write(baseline.join("keep.txt"), b"keep").unwrap();
        invalidate_manifest(&baseline).unwrap();
        assert!(!baseline.join(MATRIX_MANIFEST_FILENAME).exists());
        assert_eq!(fs::read(baseline.join("keep.txt")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(root);
    }
}

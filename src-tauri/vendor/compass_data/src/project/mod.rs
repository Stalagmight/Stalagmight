//! Compass Project
//!
//! This module provides the ability to read, write, and work with Compass project files
//! Compass project files are stored in a makefile format
//! The compass file format is documented here:
//! [Compass Project Documentation](https://www.fountainware.com/compass/HTML_Help/Project_Manager/projectfileformat.htm)
//!
pub(crate) mod lexer;
mod parser;

use crate::{EastNorthElevation, Error, Survey, UtmLocation};
use std::{
    marker::PhantomData,
    path::{Path, PathBuf},
};
use uuid::Uuid;

/// Compass projects can be defined in a variety of geodetic datums.
///
/// The datum is used to convert between the geodetic coordinates used in the survey data.
/// This enum provides a list of the datums supported by Compass.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum Datum {
    Adindan,
    Arc1950,
    Arc1960,
    Australian1966,
    Australian1984,
    CampAreaAstro,
    Cape,
    European1950,
    European1979,
    Geodetic1949,
    HongKong1963,
    HuTzuShan,
    Indian,
    NorthAmerican1927,
    NorthAmerican1983,
    Oman,
    OrdinanceSurvey1936,
    Pulkovo1942,
    SouthAmerican1956,
    SouthAmerican1969,
    Tokyo,
    WGS1972,
    WGS1984,
}

/// Declination mode for project-level declination handling
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub enum DeclinationMode {
    /// Declinations are ignored
    Ignore,
    /// Use declinations entered in the survey book
    #[default]
    Entered,
    /// Calculate declinations from survey date and geographic location
    Auto,
}

/// Project-level parameter flags from the `!` parameter in MAK files
///
/// All flags are optional and order-independent when parsing.
/// Missing flags use default values.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[allow(clippy::struct_excessive_bools)]
pub struct ProjectParameters {
    /// Whether all settings are globally overridden by project settings (G/g)
    pub global_override: bool,
    /// How declinations are derived and processed (I/E/A)
    pub declination_mode: DeclinationMode,
    /// Whether UTM convergence should be applied to the data (V/v)
    pub utm_convergence_applied: bool,
    /// Whether LRUD association settings are overridden (O/o)
    pub override_lrud_association: bool,
    /// If `override_lrud_association` is true, whether LRUDs associate with "To" station (T/t)
    pub lrud_to_station: bool,
    /// Whether shot flags are applied (S/s)
    pub shot_flags_applied: bool,
    /// Whether "X" total exclusion flags are applied (X/x)
    pub total_exclusion_applied: bool,
    /// Whether "P" plotting exclusion flags are applied (P/p)
    pub plotting_exclusion_applied: bool,
    /// Whether "L" length exclusion flags are applied (L/l)
    pub length_exclusion_applied: bool,
    /// Whether "C" close-exclusion flags are applied (C/c)
    pub close_exclusion_applied: bool,
}

/// UTM Convergence parameter from the `%` or `*` parameter in MAK files
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FileConvergence {
    /// Whether file-level convergence is enabled (`%`) or disabled (`*`)
    pub enabled: bool,
    /// The convergence angle value
    pub angle: f64,
}

/// State captured when a .dat file is encountered during project parsing.
///
/// Compass uses rolling state - each file captures whatever state was active
/// at the moment it was parsed. This allows different files in the same project
/// to have different datums, base locations, etc.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct FileState {
    /// The datum active when this file was encountered
    pub datum: Datum,
    /// The base location active when this file was encountered
    pub base_location: UtmLocation,
    /// The UTM zone for fixed stations (optional)
    pub utm_zone: Option<u8>,
    /// File-level convergence parameter (optional)
    pub file_convergence: Option<FileConvergence>,
    /// Project-level parameter flags (optional)
    pub project_parameters: Option<ProjectParameters>,
}

/// A station reference in a project file
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Station {
    /// The station name as referenced in the project (`#file,STATION[...]`).
    /// (Stalagmight fork: made public so consumers can resolve georeference anchors.)
    pub name: String,
    /// The fixed location for this station, if the project pins one.
    /// (Stalagmight fork: made public.)
    pub location: Option<EastNorthElevation>,
}

/// Marker type for survey and project files which have not been fully loaded yet
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Unloaded;
/// Marker type for survey and project files which have been fully loaded
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Loaded;

/// A survey data file (.dat) referenced by a project
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct DatFile<S> {
    /// Path to the .dat file (relative to project file)
    pub file_path: PathBuf,
    /// Stations referenced in the project file for this survey
    pub project_stations: Vec<Station>,
    /// The state that was active when this file was encountered during parsing
    pub file_state: FileState,
    /// The loaded surveys (empty until loaded)
    pub(crate) surveys: Vec<Survey>,
    /// The tail of the file the survey parser did not consume, and the offset
    /// it stopped at (both empty/zero until loaded).  (Stalagmight fork: the
    /// file parser is a `many0` and stops at the first survey it cannot read,
    /// leaving the rest of the file behind; without this the surveys after it
    /// disappear from the import with nothing said about them.)
    pub(crate) unread: String,
    pub(crate) unread_at: usize,
    pub(crate) state: PhantomData<S>,
}

impl DatFile<Unloaded> {
    /// Load the survey data file from disk
    /// Consumes the `DatFile<Unloaded>` and returns a `DatFile<Loaded>` with the survey data populated
    /// # Returns
    /// `DatFile<Loaded>` representing the contents of the survey file
    /// # Errors
    /// - [`Error::SurveyFileNotFound`] If the file does not exist
    /// - [`Error::CouldntReadFile`] If the file cannot be read
    pub fn load(self, project_path: &Path) -> Result<DatFile<Loaded>, Error> {
        let full_path = project_path.join(&self.file_path);
        if !full_path.exists() {
            return Err(Error::SurveyFileNotFound(full_path));
        }
        // Stalagmight fork: Compass files are Windows-1252, not UTF-8 (e.g. the
        // degree sign 0xB0 in comments), so decode each byte as latin1 rather
        // than failing on invalid UTF-8.
        let bytes = std::fs::read(&full_path).map_err(Error::CouldntReadFile)?;
        let file_contents: String = bytes.iter().map(|&b| b as char).collect();
        self.load_from_text(&file_contents)
    }

    /// Parse the survey data file from text the caller has already read
    /// (Stalagmight fork). Lets a consumer apply its own file-reading policy -
    /// which paths may be read, how large a file may be, how its bytes are
    /// decoded - instead of [`Self::load`]'s.
    ///
    /// The text should hold one `char` per byte of the file (a single-byte
    /// decode such as Windows-1252), so [`DatFile::unread_at`] stays a byte
    /// offset.
    /// # Errors
    /// - [`Error::CouldntParseSurvey`] If the survey file cannot be parsed
    pub fn load_from_text(self, file_contents: &str) -> Result<DatFile<Loaded>, Error> {
        let (surveys, remainder) = Survey::parse_dat_file_with_remainder(file_contents)?;
        // One decoded byte is one `char`, so the chars ahead of the remainder
        // count the bytes of the file that were read.
        let unread_at = file_contents.chars().count() - remainder.chars().count();
        let unread = remainder.to_string();
        Ok(DatFile {
            file_path: self.file_path,
            project_stations: self.project_stations,
            file_state: self.file_state,
            surveys,
            unread,
            unread_at,
            state: PhantomData,
        })
    }
}

impl DatFile<Loaded> {
    /// Get the surveys loaded from this file
    #[must_use]
    pub fn surveys(&self) -> &[Survey] {
        &self.surveys
    }

    /// The tail of the file the survey parser did not consume.
    ///
    /// Empty, or only the record separators Compass writes between surveys,
    /// when the whole file was read; anything else is the survey text the
    /// parse stopped on, and the surveys behind it are not in [`Self::surveys`].
    /// (Stalagmight fork.)
    #[must_use]
    pub fn unread(&self) -> &str {
        &self.unread
    }

    /// The byte offset into the file where the parse stopped, i.e. where
    /// [`Self::unread`] begins.  (Stalagmight fork.)
    #[must_use]
    pub fn unread_at(&self) -> usize {
        self.unread_at
    }
}

/// A Compass project file (.mak)
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
pub struct Project<S> {
    /// Optional project ID (UUID)
    pub id: Option<Uuid>,
    /// The file path to the project file on disk
    /// This is used to resolve relative paths to survey data files
    /// # Note
    /// ignored for equality checks
    pub file_path: PathBuf,
    /// The survey data files in this project
    pub survey_files: Vec<DatFile<S>>,
    pub(crate) state: PhantomData<S>,
}

impl PartialEq for Project<Unloaded> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.survey_files == other.survey_files
    }
}

impl PartialEq for Project<Loaded> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.survey_files == other.survey_files
    }
}

impl Project<Unloaded> {
    /// Read a Compass project file from disk
    /// The project file is read from disk and parsed into a `Project` struct,
    /// but this does not parse the referenced survey data files
    /// # Returns
    /// `Project<Unloaded>` representing the contents of the project file
    /// # Errors
    /// - [`Error::ProjectFileNotFound`] If the file does not exist
    /// - [`Error::CouldntReadFile`] If the file cannot be read
    /// - [`Error::CouldntParseProject`] If the file cannot be parsed
    pub fn read(file_path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = file_path.as_ref().to_path_buf();
        if !path.exists() {
            return Err(Error::ProjectFileNotFound(path));
        }
        let file_contents = std::fs::read_to_string(&path).map_err(Error::CouldntReadFile)?;
        Self::from_text(path, &file_contents)
    }

    /// Parse a project file from text the caller has already read
    /// (Stalagmight fork), so a consumer can decode the file its own way -
    /// [`Self::read`] insists on UTF-8, and a Compass project is written in
    /// the Windows ANSI code page. `file_path` is where the file lives; the
    /// survey files it lists are relative to its folder.
    /// # Errors
    /// - [`Error::CouldntParseProject`] If the file cannot be parsed
    pub fn from_text(file_path: impl AsRef<Path>, file_contents: &str) -> Result<Self, Error> {
        let path = file_path.as_ref().to_path_buf();

        // Phase 1: Tokenize
        let tokens = lexer::tokenize(file_contents)
            .map_err(|e| Error::CouldntParseProject(e.to_string()))?;

        // Phase 2: Parse
        let project = parser::parse_project(path, &tokens, file_contents)
            .map_err(|e| Error::CouldntParseProject(e.to_string()))?;

        Ok(project)
    }

    /// Load the project's survey data files through `read` (Stalagmight
    /// fork). `read` gets each file's path as the project lists it - relative
    /// to the project file's folder unless the project wrote an absolute one -
    /// and returns the file's text (see [`DatFile::load_from_text`]), or `None`
    /// to leave that file out: the consumer decides which files may be read
    /// and reports the ones it skipped, and the rest of the project still
    /// loads.
    /// # Errors
    /// - [`Error::CouldntParseSurvey`] If a survey file that was read cannot be parsed
    pub fn load_survey_files_with(
        self,
        mut read: impl FnMut(&Path) -> Option<String>,
    ) -> Result<Project<Loaded>, Error> {
        let mut survey_files = Vec::new();
        for survey_file in self.survey_files {
            let Some(text) = read(&survey_file.file_path) else {
                continue;
            };
            survey_files.push(survey_file.load_from_text(&text)?);
        }
        Ok(Project {
            id: self.id,
            file_path: self.file_path,
            survey_files,
            state: PhantomData::<Loaded>,
        })
    }

    /// Read a Compass project's survey data files from disk
    /// The data files are read from the paths specified in the project file
    /// # Returns
    /// [`Project<Loaded>`] representing the project file, complete with survey data
    /// # Errors
    /// - [`Error::SurveyFileNotFound`] If a listed survey file does not exist
    /// - [`Error::CouldntReadFile`] If the file cannot be read
    /// - [`Error::CouldntParseSurvey`] If the survey file cannot be parsed
    #[allow(clippy::missing_panics_doc)]
    pub fn load_survey_files(self) -> Result<Project<Loaded>, Error> {
        let mut survey_files = Vec::new();
        // This unwrap is safe because we know the file path existed to read this project
        // therefore the parent directory must exist
        let project_dir = self.file_path.parent().unwrap();
        for survey_file in self.survey_files {
            let survey_file = survey_file.load(project_dir)?;
            survey_files.push(survey_file);
        }
        Ok(Project {
            id: self.id,
            file_path: self.file_path,
            survey_files,
            state: PhantomData::<Loaded>,
        })
    }
}

impl Project<Loaded> {
    /// Programmatically create a new compass project
    #[must_use]
    pub fn new(project_id: Option<Uuid>, file_path: impl AsRef<Path>) -> Self {
        let file_path = file_path.as_ref().to_path_buf();
        Self {
            id: project_id,
            file_path,
            survey_files: Vec::new(),
            state: PhantomData::<Loaded>,
        }
    }

    /// Add a survey file to the project
    pub fn add_survey_file(&mut self, dat_file: DatFile<Loaded>) {
        self.survey_files.push(dat_file);
    }
}

#[cfg(test)]
mod tests {
    use crate::Error;

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn programatic_creation() {
        let new_project = Project::new(None, "Ginnie.mak");
        assert!(new_project.survey_files.is_empty());
    }

    #[test]
    fn bad_path() {
        let path = PathBuf::from("does_not_exist.mak");
        let result = Project::read(&path);
        assert!(result.is_err_and(|err| matches!(err, Error::ProjectFileNotFound(_path))));
    }

    /// Stalagmight fork: the caller reads the files. A survey file the reader
    /// declines is left out and the others still load.
    #[test]
    fn load_with_a_caller_supplied_reader() {
        let project = Project::from_text(
            "/nowhere/project.mak",
            "@1.0,2.0,3.0,13,0.0;\n&WGS 1984;\n#kept.dat;\n#skipped.dat;\n",
        )
        .unwrap();
        assert_eq!(project.survey_files.len(), 2);
        let dat = "CAVE\r\nSURVEY NAME: A\r\nSURVEY DATE: 7 10 1979\r\nSURVEY TEAM:\r\nNobody\r\n\
                   DECLINATION: 0.00  FORMAT: DDDDLRUDLADN  CORRECTIONS: 0.00 0.00 0.00\r\n\r\n\
                   FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT FLAGS COMMENTS\r\n\r\n\
                   A1 A2 10.00 90.00 0.00 1.00 1.00 1.00 1.00\r\n\x0c\r\n";
        let mut asked = Vec::new();
        let loaded = project
            .load_survey_files_with(|path| {
                asked.push(path.to_path_buf());
                (path == Path::new("kept.dat")).then(|| dat.to_string())
            })
            .unwrap();
        assert_eq!(
            asked,
            [PathBuf::from("kept.dat"), PathBuf::from("skipped.dat")],
            "the reader sees each path as the project lists it"
        );
        assert_eq!(loaded.survey_files.len(), 1);
        assert_eq!(loaded.survey_files[0].surveys().len(), 1);
    }

    #[test]
    fn parse_and_load_compass_sample() {
        let mut sample_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        sample_path.push("test_data/Fulfords.mak");

        let read_project = Project::read(&sample_path).unwrap();
        assert_eq!(read_project.survey_files.len(), 2);

        // Verify state is captured per file
        for dat_file in &read_project.survey_files {
            assert_eq!(dat_file.file_state.datum, Datum::NorthAmerican1983);
        }

        let _loaded_project = read_project.load_survey_files().unwrap();
    }
}

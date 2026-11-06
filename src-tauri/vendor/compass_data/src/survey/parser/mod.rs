mod survey_parameters;

use chrono::NaiveDate;
use nom::{
    IResult, Parser,
    bytes::complete::{tag, take_till1},
    character::complete::multispace0,
    error::{Error, ErrorKind},
    combinator::opt,
    multi::many0,
};

use crate::{
    common_types::Date,
    parser_utils::{parse_double, parse_station_name, parse_uint, recognize_line, ws},
    survey::parser::survey_parameters::parse_survey_parameters,
};

use super::{Shot, Survey};

fn parse_cave_name(input: &str) -> IResult<&str, String> {
    let (input, cave_name) = recognize_line(input)?;
    let (cave_name, _) = multispace0(cave_name)?;
    Ok((input, cave_name.to_string()))
}

fn parse_survey_name(input: &str) -> IResult<&str, String> {
    let (input, survey_line) = recognize_line(input)?;
    let (name, _) = tag("SURVEY NAME:")(survey_line)?;
    let (_, name) = parse_station_name(name)?;

    Ok((input, name.to_string()))
}

fn parse_survey_date_line(input: &str) -> IResult<&str, (Date, Option<String>)> {
    let (input, date_line) = recognize_line(input)?;
    let (date_line, _) = tag("SURVEY DATE:")(date_line)?;
    let (date_line, month) = parse_uint(date_line)?;
    let (date_line, day) = parse_uint(date_line)?;
    let (date_line, year) = parse_uint(date_line)?;
    let comment = match tag::<&str, &str, Error<&str>>("COMMENT:")(date_line) {
        Ok((comment, _)) => Some(comment.to_string()),
        Err(_unused) => None,
    };
    #[allow(clippy::cast_possible_truncation)]
    let date = Date {
        month: month as u8,
        day: day as u8,
        year: year as u16,
    };
    Ok((input, (date, comment)))
}

fn parse_survey_team(input: &str) -> IResult<&str, String> {
    // (Stalagmight fork, #576 FZ-PARSE-6.) The team is on the line after the
    // tag, and Compass leaves that line empty for a survey with no team.
    // Upstream skipped `multispace1` here, which swallowed the empty team line
    // too: the parameter line was read as the team and the survey (and every
    // one after it) failed to parse. A team written on the tag's own line is
    // still read from there.
    let (input, _) = tag("SURVEY TEAM:")(input)?;
    let (mut input, rest_of_tag_line) = recognize_line(input)?;
    if !rest_of_tag_line.trim().is_empty() {
        return Ok((input, rest_of_tag_line.trim_start().to_string()));
    }
    // (#576 RVF-b.) Blank lines before the team line are skipped, as
    // upstream's `multispace1` skipped them: a team written after one is
    // still the team, and the surveys after it still read. The survey has no
    // team when the next non-blank line is its parameter line, which is left
    // for `parse_survey_parameters`.
    loop {
        let (rest, line) = recognize_line(input)?;
        if line.starts_with("DECLINATION:") {
            return Ok((input, String::new()));
        }
        if !line.trim().is_empty() {
            return Ok((rest, line.trim_start().to_string()));
        }
        input = rest;
    }
}

fn gobble_labels(input: &str) -> IResult<&str, &str> {
    let (input, _) = ws(tag("FROM")).parse(input)?;
    let (input, _) = ws(take_till1(|c| c == '\n')).parse(input)?;
    Ok((input, ""))
}

/// Split the trailing portion of a shot line into an optional flag string and
/// an optional comment. Compass writes flags as `#|CHARS#`; anything outside
/// that token (before or after) is free-form comment text. (Stalagmight fork
/// addition — upstream discards the line tail.)
fn split_flags_and_comment(tail: &str) -> (Option<String>, Option<String>) {
    let tail = tail.trim();
    if tail.is_empty() {
        return (None, None);
    }
    if let Some(start) = tail.find("#|") {
        let after = &tail[start + 2..];
        if let Some(end) = after.find('#') {
            let flags = after[..end].trim();
            let before = tail[..start].trim();
            let rest = after[end + 1..].trim();
            let comment = match (before.is_empty(), rest.is_empty()) {
                (true, true) => None,
                (true, false) => Some(rest.to_string()),
                (false, true) => Some(before.to_string()),
                (false, false) => Some(format!("{before} {rest}")),
            };
            let flags = if flags.is_empty() {
                None
            } else {
                Some(flags.to_string())
            };
            return (flags, comment);
        }
    }
    (None, Some(tail.to_string()))
}

fn parse_shot(input: &str, has_backsights: bool) -> IResult<&str, Shot> {
    let (input, line) = recognize_line(input)?;
    let (line, from) = parse_station_name(line)?;
    let (line, to) = parse_station_name(line)?;
    let (line, length) = parse_double(line)?;
    let (line, azimuth) = parse_double(line)?;
    let (line, inclination) = parse_double(line)?;
    let (line, left) = parse_double(line)?;
    let (line, up) = parse_double(line)?;
    let (line, down) = parse_double(line)?;
    let (line, right) = parse_double(line)?;
    let (line, back_azimuth, back_inclination) = if has_backsights {
        let (line, back_azimuth) = parse_double(line)?;
        let (line, back_inclination) = parse_double(line)?;
        (line, Some(back_azimuth), Some(back_inclination))
    } else {
        (line, None, None)
    };
    let (flags, comment) = split_flags_and_comment(line);
    let shot = Shot {
        from: from.to_string(),
        to: to.to_string(),
        length,
        azimuth,
        inclination,
        up,
        down,
        left,
        right,
        back_azimuth,
        back_inclination,
        flags,
        comment,
    };
    Ok((input, shot))
}

pub(crate) fn parse_survey(input: &str) -> IResult<&str, Survey> {
    let (input, cave_name) = parse_cave_name(input)?;
    let (input, name) = parse_survey_name(input)?;
    let (input, (date, comment)) = parse_survey_date_line(input)?;
    // `SURVEY DATE:` is three unsigned integers and nothing in the format says
    // they name a real day, so `2 29 1979` and `13 45 1979` both get this far.
    // A survey whose date does not exist is a survey this parser cannot
    // produce, which is a parse failure like any other: `many0` stops and the
    // caller sees a short file. It is emphatically NOT a panic - the callers
    // are Tauri commands, and a panic there never reaches the user as an error.
    let Some(date) =
        NaiveDate::from_ymd_opt(i32::from(date.year), u32::from(date.month), u32::from(date.day))
    else {
        return Err(nom::Err::Error(Error::new(input, ErrorKind::Verify)));
    };
    let (input, team) = parse_survey_team(input)?;
    let (input, parameters) = parse_survey_parameters(input)?;
    let (input, _) = gobble_labels(input)?;
    let has_backsights = parameters.format_parameters.has_backsights();
    let (input, shots) = many0(move |i| parse_shot(i, has_backsights)).parse(input)?;
    let (input, _) = opt(ws(tag(""))).parse(input)?;
    Ok((
        input,
        Survey {
            cave_name,
            name,
            date,
            comment,
            team,
            parameters,
            shots,
        },
    ))
}

pub fn parse_dat_file(input: &str) -> IResult<&str, Vec<Survey>> {
    many0(parse_survey).parse(input)
}

#[cfg(test)]
mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parse_example_data() {
        let input = include_str!("../../../test_data/Fulford.dat");
        let (_input, surveys) = many0(parse_survey).parse(input).unwrap();

        // Test round-trip: parse -> serialize -> parse should give same result
        for survey in &surveys {
            let serialized = survey.serialize();
            let (_, reparsed_surveys) = many0(parse_survey).parse(&serialized).unwrap();
            assert_eq!(reparsed_surveys.len(), 1, "Should parse back to one survey");
            assert_eq!(
                &reparsed_surveys[0], survey,
                "Round-trip for survey '{}' failed",
                survey.name
            );
        }
    }

    #[test]
    fn dat_file_round_trip() {
        // Test that we can parse and re-serialize an entire .dat file
        let input = include_str!("../../../test_data/Fulsurf.dat");
        let (_, surveys) = parse_dat_file(input).unwrap();

        for survey in &surveys {
            let serialized = survey.serialize();
            let (_, reparsed) = many0(parse_survey).parse(&serialized).unwrap();
            assert_eq!(reparsed.len(), 1);
            assert_eq!(
                &reparsed[0], survey,
                "Round-trip for survey '{}' failed",
                survey.name
            );
        }
    }

    /// (Stalagmight fork, #576 FZ-PARSE-6.) Compass writes the team on the line
    /// after `SURVEY TEAM:` and leaves that line empty for a survey with no
    /// team, and so does this crate's own `serialize`. The empty line used to
    /// be swallowed, the parameter line read as the team, and that survey and
    /// every one after it dropped. A team on the tag's own line still reads.
    #[test]
    fn a_survey_with_no_team_reads_back() {
        let text = "CAVE\r\nSURVEY NAME: A\r\nSURVEY DATE: 7 10 1979\r\nSURVEY TEAM:\r\n\
                    Ann\r\nDECLINATION: 0.00\r\n\r\n\
                    FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n\
                    A1 A2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\x0c\r\n";
        let (_, parsed) = parse_dat_file(text).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].team, "Ann");

        let mut no_team = parsed[0].clone();
        no_team.team = String::new();
        let mut second = parsed[0].clone();
        second.name = "B".to_string();
        let file = format!("{}{}", no_team.serialize(), second.serialize());
        let (_, back) = parse_dat_file(&file).unwrap();
        assert_eq!(back, vec![no_team, second]);

        let same_line = text.replace("SURVEY TEAM:\r\nAnn\r\n", "SURVEY TEAM: Ann\r\n");
        let (_, parsed) = parse_dat_file(&same_line).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].team, "Ann");
    }

    /// (Stalagmight fork, #576 RVF-b.) A team written after a blank line
    /// used to read as no team, and the team line then failed the parameter
    /// line: that survey and every later one were dropped. Neither Compass nor
    /// `serialize` writes this layout, but a reader must not lose the rest of
    /// the file to it. The survey after it has no team at all.
    #[test]
    fn a_team_after_a_blank_line_reads_and_later_surveys_survive() {
        let survey = |name: &str, team_lines: &str| {
            format!(
                "CAVE\r\nSURVEY NAME: {name}\r\nSURVEY DATE: 7 10 1979\r\nSURVEY TEAM:\r\n\
                 {team_lines}DECLINATION: 0.00\r\n\r\n\
                 FROM TO LENGTH BEARING INC LEFT UP DOWN RIGHT\r\n\r\n\
                 {name}1 {name}2 10.00 90.00 0.00 1.00 2.00 3.00 4.00\r\n\x0c\r\n"
            )
        };
        let text = [
            survey("A", "\r\n\r\nAnn\r\n"),
            survey("B", "\r\n"),
            survey("C", "Cy\r\n"),
        ]
        .concat();
        let (_, parsed) = parse_dat_file(&text).unwrap();
        let read: Vec<(&str, &str)> = parsed
            .iter()
            .map(|s| (s.name.as_str(), s.team.as_str()))
            .collect();
        assert_eq!(read, [("A", "Ann"), ("B", ""), ("C", "Cy")]);
    }

    #[test]
    fn test_parse_dat_file_returns_surveys() {
        // Verify that parse_dat_file actually returns surveys (not an empty vec)
        let input = include_str!("../../../test_data/Fulford.dat");
        let (_, surveys) = parse_dat_file(input).unwrap();
        assert_eq!(surveys.len(), 25, "Should parse all 25 surveys from Fulford.dat");
    }

    #[test]
    fn test_read_to_string_matches_include_str() {
        // Verify that read_to_string produces the same result as include_str
        let include_input = include_str!("../../../test_data/Fulford.dat");
        let read_input = std::fs::read_to_string("test_data/Fulford.dat").unwrap();

        let (_, include_surveys) = parse_dat_file(include_input).unwrap();
        let (_, read_surveys) = parse_dat_file(&read_input).unwrap();

        assert_eq!(
            include_surveys.len(),
            read_surveys.len(),
            "Should parse same number of surveys"
        );
    }

    // Edge case tests
    #[test]
    fn test_parse_survey_no_shots() {
        let input = "Test Cave\n\
                     SURVEY NAME: A\n\
                     SURVEY DATE: 1 15 2024\n\
                     SURVEY TEAM:\n\
                     John Doe\n\
                     DECLINATION: 10.0\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                     \x0c\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert_eq!(survey.cave_name, "Test Cave");
        assert_eq!(survey.name, "A");
        assert!(survey.shots.is_empty());
    }

    #[test]
    fn test_parse_survey_with_comment() {
        let input = "Test Cave\n\
                     SURVEY NAME: B\n\
                     SURVEY DATE: 6 29 1987  COMMENT:This is a test comment\n\
                     SURVEY TEAM:\n\
                     Jane Doe\n\
                     DECLINATION: 11.5\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                     \x0c\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert_eq!(survey.comment, Some("This is a test comment".to_string()));
    }

    #[test]
    fn test_parse_survey_special_station_names() {
        let input = "Test Cave\n\
                     SURVEY NAME: C\n\
                     SURVEY DATE: 1 1 2000\n\
                     SURVEY TEAM:\n\
                     Team\n\
                     DECLINATION: 0.0\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                            A_1          A-2     10.0     90.0      0.0      1.0      2.0      3.0      4.0\n\
                           A'3          A*4     15.0    180.0     -5.0      2.0      3.0      4.0      5.0\n\
                     \x0c\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert_eq!(survey.shots.len(), 2);
        assert_eq!(survey.shots[0].from, "A_1");
        assert_eq!(survey.shots[0].to, "A-2");
        assert_eq!(survey.shots[1].from, "A'3");
        assert_eq!(survey.shots[1].to, "A*4");
    }

    #[test]
    fn test_parse_survey_negative_values() {
        let input = "Test Cave\n\
                     SURVEY NAME: D\n\
                     SURVEY DATE: 12 31 1999\n\
                     SURVEY TEAM:\n\
                     Team\n\
                     DECLINATION: -5.5\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                              A1           A2     10.0    270.0    -45.0     -1.0     -2.0     -3.0     -4.0\n\
                     \x0c\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert!((survey.parameters.declination - (-5.5)).abs() < 0.001);
        assert!((survey.shots[0].inclination - (-45.0)).abs() < 0.001);
    }

    #[test]
    fn test_parse_survey_missing_data_marker() {
        // -9999.0 is used to indicate missing LRUD data in Compass files
        let input = "Test Cave\n\
                     SURVEY NAME: E\n\
                     SURVEY DATE: 6 15 2020\n\
                     SURVEY TEAM:\n\
                     Team\n\
                     DECLINATION: 10.0\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                              A1           A2     10.0     90.0      0.0  -9999.0  -9999.0  -9999.0  -9999.0\n\
                     \x0c\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert!((survey.shots[0].left - (-9999.0)).abs() < 0.001);
        assert!((survey.shots[0].up - (-9999.0)).abs() < 0.001);
    }

    #[test]
    fn test_parse_survey_crlf_line_endings() {
        let input = "Test Cave\r\n\
                     SURVEY NAME: F\r\n\
                     SURVEY DATE: 3 14 2023\r\n\
                     SURVEY TEAM:\r\n\
                     Team\r\n\
                     DECLINATION: 0.0\r\n\
                     \r\n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\r\n\
                     \r\n\
                     \x0c\r\n";
        let (_, survey) = parse_survey(input).unwrap();
        assert_eq!(survey.cave_name, "Test Cave");
        assert_eq!(survey.name, "F");
    }

    #[test]
    fn test_parse_cave_name() {
        let (remaining, name) = parse_cave_name("My Cave Name\nrest").unwrap();
        assert_eq!(name, "My Cave Name");
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_survey_name() {
        let (remaining, name) = parse_survey_name("SURVEY NAME: ABC123\nrest").unwrap();
        assert_eq!(name, "ABC123");
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_survey_date_without_comment() {
        let (remaining, (date, comment)) =
            parse_survey_date_line("SURVEY DATE: 6 29 1987\nrest").unwrap();
        assert_eq!(date.month, 6);
        assert_eq!(date.day, 29);
        assert_eq!(date.year, 1987);
        assert!(comment.is_none());
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_survey_date_with_comment() {
        let (remaining, (date, comment)) =
            parse_survey_date_line("SURVEY DATE: 12 25 2000  COMMENT:Holiday survey\nrest").unwrap();
        assert_eq!(date.month, 12);
        assert_eq!(date.day, 25);
        assert_eq!(date.year, 2000);
        assert_eq!(comment, Some("Holiday survey".to_string()));
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_survey_team() {
        let (remaining, team) = parse_survey_team("SURVEY TEAM:\nAlice,Bob,Charlie\nrest").unwrap();
        assert_eq!(team, "Alice,Bob,Charlie");
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_shot() {
        let input = "          A1           A2     21.75    63.50   -28.00      2.60      2.60      2.60      2.60\nrest";
        let (remaining, shot) = parse_shot(input, false).unwrap();
        assert_eq!(shot.from, "A1");
        assert_eq!(shot.to, "A2");
        assert!((shot.length - 21.75).abs() < 0.001);
        assert!((shot.azimuth - 63.50).abs() < 0.001);
        assert!((shot.inclination - (-28.0)).abs() < 0.001);
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_shot_with_flags_and_comment() {
        // Frontsight-only line with a flag token and trailing comment.
        let input = "  A2  A3   41.17   46.00   2.00  0.00  0.00  0.00  0.00 #|PC# Big Room\nrest";
        let (remaining, shot) = parse_shot(input, false).unwrap();
        assert_eq!(shot.from, "A2");
        assert_eq!(shot.flags.as_deref(), Some("PC"));
        assert_eq!(shot.comment.as_deref(), Some("Big Room"));
        assert!(shot.back_azimuth.is_none());
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_shot_with_backsights() {
        // Backsight columns (315.0 -5.0) must be captured, not folded into the comment.
        let input = "  A2  A1   12.00  135.00   5.00  0.00  4.00  0.50  0.00 315.0 -5.0 Big Room\nrest";
        let (remaining, shot) = parse_shot(input, true).unwrap();
        assert!((shot.back_azimuth.unwrap_or(0.0) - 315.0).abs() < 0.001);
        assert!((shot.back_inclination.unwrap_or(0.0) - (-5.0)).abs() < 0.001);
        assert_eq!(shot.comment.as_deref(), Some("Big Room"));
        assert_eq!(remaining, "rest");
    }

    #[test]
    fn test_parse_multiple_surveys_in_file() {
        let input = "Cave One\n\
                     SURVEY NAME: A\n\
                     SURVEY DATE: 1 1 2020\n\
                     SURVEY TEAM:\n\
                     Team A\n\
                     DECLINATION: 10.0\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                     \x0c\n\
                     Cave Two\n\
                     SURVEY NAME: B\n\
                     SURVEY DATE: 2 2 2021\n\
                     SURVEY TEAM:\n\
                     Team B\n\
                     DECLINATION: 11.0\n\
                     \n\
                            FROM           TO   LENGTH  BEARING      INC     LEFT       UP     DOWN    RIGHT\n\
                     \n\
                     \x0c\n";
        let (_, surveys) = parse_dat_file(input).unwrap();
        assert_eq!(surveys.len(), 2);
        assert_eq!(surveys[0].cave_name, "Cave One");
        assert_eq!(surveys[0].name, "A");
        assert_eq!(surveys[1].cave_name, "Cave Two");
        assert_eq!(surveys[1].name, "B");
    }
}

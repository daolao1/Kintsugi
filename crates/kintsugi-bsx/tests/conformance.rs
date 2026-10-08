//! The seam contract, run against the second engine.
//!
//! `kintsugi-testkit` holds the rules that are true of every engine — a name is
//! never evidence, `Certain` means mountable, a seam names itself, an empty
//! folder is nobody's game, and changing nothing changes nothing. Running them
//! against a second, unrelated format family is what turns the contract from
//! BlueGale's description into the body's: this seam was written against a real
//! 2008 release, by the same tool, with the same rules, and had to satisfy them
//! without changing them.
//!
//! The fixture is built in memory, byte by byte: no commercial game data is in
//! this repository.

use kintsugi_bsx::fixtures::{demo_release, make_bsx_dat};
use kintsugi_core::detect::Confidence;
use kintsugi_testkit::{SeamFixture, assert_seam_contract};

/// A whole BSX release: the configuration, one archive, one loose picture, and
/// the compiled story file the engine reads.
fn a_bsx_release() -> SeamFixture {
    let (archive, loose, config) = demo_release();
    SeamFixture::new(
        "a BSX release: BSX configuration, a BSArc archive, a loose BSG picture",
        Confidence::Certain,
    )
    .file("exe/bsx.ini", config)
    .file("exe/graphics.bsa", archive)
    .file("exe/title.bsg", loose)
    // The story file: one line per command, in the story's own order, so the
    // ids a translation carries are the indices the engine indexes the table
    // by and a repair lands on the line it names.
    .file("exe/bsx.dat", make_bsx_dat())
    .script("exe/bsx.dat")
}

/// The story alone, with no archive in sight.
///
/// This engine plays a game out of a compiled story; its archives are where the
/// pictures live. A seam that could only mount a game with an archive in it
/// would refuse the release that shipped its assets loose.
fn a_story_without_an_archive() -> SeamFixture {
    SeamFixture::new(
        "a BSX release whose story is the only file this seam knows",
        Confidence::Likely,
    )
    .file("exe/bsx.dat", make_bsx_dat())
    .script("exe/bsx.dat")
}

#[test]
fn the_bsx_seam_keeps_the_seam_contract() {
    assert_seam_contract(
        &kintsugi_bsx::plugin(),
        &[a_bsx_release(), a_story_without_an_archive()],
    );
}

#[test]
fn a_story_that_is_not_a_story_is_never_called_one() {
    // The magic alone is not evidence enough to hand a game's script over: a
    // file that starts with `BSScript` and whose table of lines does not parse
    // is skipped, named in a mount note, and never named as the main script.
    let mut broken = make_bsx_dat();
    let story = kintsugi_bsx::Story::parse(&broken).expect("the fixture is a story");
    broken.truncate(story.bytes().len() - 8);
    let fixture = SeamFixture::new(
        "a folder whose only BSScript file cannot be read",
        Confidence::Likely,
    )
    .file("exe/bsx.dat", broken);
    assert_seam_contract(&kintsugi_bsx::plugin(), &[fixture]);
}

#[test]
fn a_bare_archive_is_recognized_but_not_claimed_as_one_engine() {
    // BlueGale and Bishop shipped the same container. A folder that has only
    // archives must be reported as what it is: the container, mounted, with the
    // engine left open — not as a certainty this seam cannot back.
    let (archive, _, _) = demo_release();
    assert_seam_contract(
        &kintsugi_bsx::plugin(),
        &[SeamFixture::new("a bare BSArc archive", Confidence::Likely)
            .file("graphics.bsa", archive)],
    );
}

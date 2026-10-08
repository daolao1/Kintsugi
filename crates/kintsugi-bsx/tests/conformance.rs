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
    // The story file is here because detection reads its magic, and because a
    // release without one is not this engine's game. It is deliberately *not*
    // declared with `.script(...)`: this seam cannot read BSScript into the
    // body's IR yet, and a fixture that declared one would be asking for a
    // promise the seam does not make.
    .file("exe/bsx.dat", make_bsx_dat())
}

#[test]
fn the_bsx_seam_keeps_the_seam_contract() {
    assert_seam_contract(&kintsugi_bsx::plugin(), &[a_bsx_release()]);
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

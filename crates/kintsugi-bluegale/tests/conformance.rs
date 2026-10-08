//! The seam contract, run against the golden seam.
//!
//! `kintsugi-testkit` holds the rules that are true of every engine — a name is
//! never evidence, `Certain` means mountable, a seam names itself, an empty
//! folder is nobody's game, and changing nothing changes nothing. This file
//! feeds BlueGale's own format to those rules, so the contract is not a
//! description that the seams happen to satisfy: it is a test they pass.
//!
//! The fixture is built in memory, byte by byte, like every other fixture in
//! this workspace: no commercial game data is in this repository.

use kintsugi_bluegale::fixtures::{make_bdt, make_inx, make_snn};
use kintsugi_core::detect::Confidence;
use kintsugi_testkit::{SeamFixture, assert_seam_contract};

/// A whole BlueGale release: an INX index, the SNN archive it indexes, and a
/// BDT script beside them.
fn demo_release() -> SeamFixture {
    let story = make_bdt("$start\r\n深夜の工房。\r\n%fin\r\n");
    let (snn, placements) = make_snn(&[&story]);
    let (offset, size) = placements[0];
    // The archive's record says STORY.BDT, so the archive holds the script;
    // `story.bdt` below is the loose copy the same release ships. Both are
    // valid places for a BDT to be, which is why the seam sees one script.
    let index = make_inx(&[("STORY.BDT", offset, size)]);
    SeamFixture::new(
        "an INX index and its sibling SNN archive",
        Confidence::Certain,
    )
    .file("game.inx", index)
    .file("game.snn", snn)
    .file(
        "story.bdt",
        make_bdt("$start\r\n祖母の形見の茶碗。\r\n%fin\r\n"),
    )
    .script("story.bdt")
}

#[test]
fn the_bluegale_seam_keeps_the_seam_contract() {
    assert_seam_contract(&kintsugi_bluegale::plugin(), &[demo_release()]);
}

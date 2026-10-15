import Foundation
import Testing

@testable import LightCraftCore

/// The photo screen's arrangement for each size and pose (the sizes are an iPhone Duo's: outer display 466 x 678 pt
/// with a side column of about 84 pt, inner display 951 x 669 pt).
struct PhotoLayoutTests {
    private func choose(_ w: CGFloat, _ h: CGFloat, fold: DuoFold? = nil, regular: Bool = false, compactHeight: Bool = false, inspector: Bool = true) -> PhotoLayout {
        PhotoLayout.choose(room: CGSize(width: w, height: h), fold: fold, regular: regular, compactHeight: compactHeight, inspector: inspector)
    }

    @Test func aPhoneHeldUprightStacksTheTools() {
        #expect(choose(402, 800) == .stack(gap: 0, below: nil))
        // the Duo's outer display, held upright: 382 pt of room beside its side column
        #expect(choose(382, 562, inspector: false) == .stack(gap: 0, below: nil))
    }

    @Test func aPhoneHeldSidewaysKeepsItsColumn() {
        #expect(choose(874, 402, compactHeight: true) == .column(width: 316, gap: 0))
    }

    @Test func theDuosOuterDisplayOnItsSideGetsASlimmerColumnAndKeepsAPhotoWorthLookingAt() {
        // 678 pt wide less the side column: 594 pt, two fifths is 238, so the column is its least, 260
        #expect(choose(594, 380, compactHeight: true, inspector: false) == .column(width: 260, gap: 0))
    }

    @Test func aRoomTooNarrowForAColumnStacksInstead() {
        // 500 pt less a 260 pt column leaves a 240 pt photo: under the least
        #expect(choose(500, 300, compactHeight: true) == .stack(gap: 0, below: nil))
    }

    @Test func anIPadGetsTheInspector() {
        #expect(choose(834, 1194, regular: true) == .inspector)
        #expect(choose(1194, 834, regular: true) == .inspector)
        // an iPad mini upright
        #expect(choose(744, 1133, regular: true) == .inspector)
    }

    @Test func aDeviceWithAHingeKeepsTheSameColumnFlatAsItWillHaveFolded() {
        // a Duo's inner display held flat in landscape: 951 pt less the side column
        #expect(choose(867, 669, regular: true, inspector: false) == .column(width: 340, gap: 0))
    }

    @Test func aRegularWindowTooNarrowForToolsBesideThePhotoStacksInstead() {
        // the Duo's inner display held upright, and an iPad in half of Split View
        #expect(choose(669, 850, regular: true, inspector: false) == .stack(gap: 0, below: nil))
        #expect(choose(597, 800, regular: true) == .stack(gap: 0, below: nil))
    }

    @Test func aBookGivesThePhotoOnePageAndTheToolsTheOther() {
        // a hinge strip 40 pt wide, centred on the middle of the 951 pt display (475), in 867 pt of room
        let fold = DuoFold(frame: CGRect(x: 455, y: 0, width: 40, height: 669), vertical: true)
        // the photo keeps 0 to 455; the tools get everything from 495
        #expect(choose(867, 669, fold: fold, regular: true, inspector: false) == .column(width: 372, gap: 40))
    }

    @Test func aBookIsHeldEvenWhenTheScreenIsSideways() {
        let fold = DuoFold(frame: CGRect(x: 455, y: 0, width: 40, height: 669), vertical: true)
        #expect(choose(867, 669, fold: fold, regular: true, compactHeight: true, inspector: false) == .column(width: 372, gap: 40))
    }

    @Test func aHingeTooCloseToAnEdgeForTwoPagesIsIgnored() {
        // 27 pt for the tools: not a page
        let late = DuoFold(frame: CGRect(x: 800, y: 0, width: 40, height: 669), vertical: true)
        #expect(choose(867, 669, fold: late, regular: true, inspector: false) == .column(width: 340, gap: 0))
        // 100 pt for the photo: not a page
        let early = DuoFold(frame: CGRect(x: 80, y: 0, width: 40, height: 669), vertical: true)
        #expect(choose(867, 669, fold: early, regular: true, inspector: false) == .column(width: 340, gap: 0))
    }

    @Test func aTabletopGivesThePhotoTheUpperPageAndTheToolsTheLowerOne() {
        // upright, 669 pt wide, with 850 pt under the navigation bar; the hinge strip is 40 pt tall at 380
        let fold = DuoFold(frame: CGRect(x: 0, y: 380, width: 669, height: 40), vertical: false)
        #expect(choose(669, 850, fold: fold, regular: true, inspector: false) == .stack(gap: 40, below: 430))
    }

    @Test func aTabletopWithNoRoomForTheToolsBelowIsIgnored() {
        let fold = DuoFold(frame: CGRect(x: 0, y: 700, width: 669, height: 40), vertical: false)
        #expect(choose(669, 850, fold: fold, regular: true, inspector: false) == .stack(gap: 0, below: nil))
    }

    @Test func aRoomNotMeasuredYetDoesNotCrashOrInventAColumn() {
        #expect(choose(0, 0, regular: true) == .stack(gap: 0, below: nil))
        #expect(choose(0, 0, compactHeight: true) == .stack(gap: 0, below: nil))
        let fold = DuoFold(frame: CGRect(x: 10, y: 0, width: 40, height: 100), vertical: true)
        #expect(choose(0, 0, fold: fold, regular: true) == .stack(gap: 0, below: nil))
    }

    @Test func theLayoutsNameThemselvesForAutomation() {
        #expect(PhotoLayout.inspector.name == "inspector")
        #expect(PhotoLayout.column(width: 1, gap: 0).name == "column" && PhotoLayout.column(width: 1, gap: 0).isColumn)
        #expect(PhotoLayout.stack(gap: 0, below: nil).name == "stack" && !PhotoLayout.stack(gap: 0, below: nil).isColumn)
    }
}

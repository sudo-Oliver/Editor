//! Checks the colors of a rendered page. Skipped when PDFium is missing.

use std::path::Path;

use pdf::{PageRequest, PdfEvent, PdfWorker};

/// One page filled with pure red. PDFium rebuilds the missing cross references.
const RED_PAGE: &[u8] = b"%PDF-1.4
1 0 obj <</Type/Catalog/Pages 2 0 R>> endobj
2 0 obj <</Type/Pages/Kids[3 0 R]/Count 1>> endobj
3 0 obj <</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]/Contents 4 0 R>> endobj
4 0 obj <</Length 26>> stream
1 0 0 rg 0 0 10 10 re f
endstream endobj
trailer <</Root 1 0 R>>
";

fn first_pixel(invert: bool) -> Option<[u8; 4]> {
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/pdfium/lib");
    if !library.is_dir() {
        return None;
    }
    let (worker, mut events) = PdfWorker::spawn(&library).unwrap();
    worker.load(1, RED_PAGE.to_vec());
    assert!(matches!(events.blocking_recv(), Some(PdfEvent::Loaded { .. })));
    worker.render(1, vec![PageRequest { page: 0, width_px: 10, invert }]);
    match events.blocking_recv().unwrap() {
        PdfEvent::Rendered { bgra, .. } => Some(bgra[..4].try_into().unwrap()),
        other => panic!("{other:?}"),
    }
}

#[test]
fn pages_arrive_as_bgra() {
    let Some(pixel) = first_pixel(false) else {
        return;
    };
    assert_eq!(pixel, [0, 0, 255, 255]);
}

#[test]
fn inverting_keeps_the_channel_order() {
    let Some(pixel) = first_pixel(true) else {
        return;
    };
    assert_eq!(pixel, [255, 255, 0, 255]);
}

//! Checks the colors of a rendered page. Skipped when PDFium is missing.

use std::path::Path;

use pdf::{PageRequest, PdfEvent, PdfWorker, Recolor};

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

/// White paper with a green 1x1 image stretched over the left half.
fn image_page() -> Vec<u8> {
    let mut content = b"q 5 0 0 10 0 0 cm BI /W 1 /H 1 /CS /RGB /BPC 8 ID ".to_vec();
    content.extend([40, 160, 60]);
    content.extend(b" EI Q");
    let mut pdf = b"%PDF-1.4
1 0 obj <</Type/Catalog/Pages 2 0 R>> endobj
2 0 obj <</Type/Pages/Kids[3 0 R]/Count 1>> endobj
3 0 obj <</Type/Page/Parent 2 0 R/MediaBox[0 0 10 10]/Contents 4 0 R>> endobj
"
    .to_vec();
    pdf.extend(format!("4 0 obj <</Length {}>> stream\n", content.len()).as_bytes());
    pdf.extend(content);
    pdf.extend(b"\nendstream endobj\ntrailer <</Root 1 0 R>>\n");
    pdf
}

/// Black paper and white ink, which only inverts the lightness.
const NEGATIVE: Recolor = Recolor { paper: 0x000000, ink: 0xffffff };

fn render(bytes: Vec<u8>, recolor: Option<Recolor>) -> Option<Vec<u8>> {
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) { "../../vendor/pdfium/bin" } else { "../../vendor/pdfium/lib" });
    if !library.is_dir() {
        return None;
    }
    let (worker, mut events) = PdfWorker::spawn(&library).unwrap();
    worker.load(1, bytes);
    assert!(matches!(events.blocking_recv(), Some(PdfEvent::Loaded { .. })));
    worker.render(1, vec![PageRequest { page: 0, width_px: 10, recolor }]);
    match events.blocking_recv().unwrap() {
        PdfEvent::Rendered { bgra, .. } => Some(bgra),
        other => panic!("{other:?}"),
    }
}

#[test]
fn pages_arrive_as_bgra() {
    let Some(bgra) = render(RED_PAGE.to_vec(), None) else {
        return;
    };
    assert_eq!(bgra[..4], [0, 0, 255, 255]);
}

#[test]
fn dark_pages_keep_the_hue_and_the_channel_order() {
    let Some(bgra) = render(RED_PAGE.to_vec(), Some(NEGATIVE)) else {
        return;
    };
    assert_eq!(bgra[..4], [149, 149, 255, 255]);
}

#[test]
fn dark_pages_leave_images_alone() {
    let Some(plain) = render(image_page(), None) else {
        return;
    };
    let dark = render(image_page(), Some(NEGATIVE)).unwrap();
    let row = 5 * 10 * 4;
    assert_ne!(plain[row + 4..row + 8], [255, 255, 255, 255]);
    // The image on the left keeps its pixels, the paper on the right turns black.
    assert_eq!(dark[row + 4..row + 8], plain[row + 4..row + 8]);
    assert_eq!(dark[row + 32..row + 36], [0, 0, 0, 255]);
}

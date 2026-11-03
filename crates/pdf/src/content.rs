//! The text and links of a page, for selecting and clicking. Coordinates are PDF
//! points with the origin at the top left of the page, like the rest of the app.

use pdfium_render::prelude::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Rect {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }

    /// PDF coordinates start at the bottom left of the page.
    fn from_pdf(rect: &PdfRect, page_height: f32) -> Self {
        Self {
            left: rect.left().value,
            top: page_height - rect.top().value,
            right: rect.right().value,
            bottom: page_height - rect.bottom().value,
        }
    }
}

/// `rect` is empty for characters that PDFium generated, such as line breaks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageChar {
    pub ch: char,
    pub rect: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkTarget {
    Page { page: usize, y: Option<f32> },
    Uri(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct PageLink {
    pub rect: Rect,
    pub target: LinkTarget,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageContent {
    pub chars: Box<[PageChar]>,
    pub links: Box<[PageLink]>,
}

/// `heights` holds the height of every page, because link targets can be on any page.
pub(crate) fn extract(page: &PdfPage, heights: &[f32]) -> Result<PageContent, PdfiumError> {
    let height = page.height().value;
    let text = page.text()?;
    let chars: Box<[PageChar]> = text
        .chars()
        .iter()
        .filter_map(|character| {
            let rect = character.loose_bounds().map_or(Rect::default(), |rect| Rect::from_pdf(&rect, height));
            Some(PageChar { ch: character.unicode_char()?, rect })
        })
        .collect();
    let links = page
        .links()
        .iter()
        .filter_map(|link| {
            Some(PageLink {
                rect: Rect::from_pdf(&link.rect().ok()?, height),
                target: link_target(&link, heights)?,
            })
        })
        .collect();
    Ok(PageContent { chars, links })
}

fn link_target(link: &PdfLink, heights: &[f32]) -> Option<LinkTarget> {
    if let Some(destination) = link.destination() {
        return page_target(&destination, heights);
    }
    match link.action()? {
        PdfAction::LocalDestination(action) => page_target(&action.destination().ok()?, heights),
        PdfAction::Uri(action) => Some(LinkTarget::Uri(action.uri().ok()?)),
        _ => None,
    }
}

fn page_target(destination: &PdfDestination, heights: &[f32]) -> Option<LinkTarget> {
    let page = usize::from(destination.page_index().ok()?);
    let height = *heights.get(page)?;
    let y = match destination.view_settings().ok()? {
        PdfDestinationViewSettings::SpecificCoordinatesAndZoom(_, y, _) => y,
        PdfDestinationViewSettings::FitPageHorizontallyToWindow(y) => y,
        PdfDestinationViewSettings::FitPageToRectangle(rect) => Some(rect.top()),
        _ => None,
    };
    Some(LinkTarget::Page {
        page,
        y: y.map(|y| height - y.value),
    })
}

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
}

/// Maps PDF coordinates to the page as it appears on screen. PDF coordinates start
/// at the bottom left of the media box and ignore the rotation of the page, which
/// pdflscape uses for landscape pages.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PageFrame {
    left: f32,
    bottom: f32,
    width: f32,
    height: f32,
    rotation: PdfPageRenderRotation,
}

impl PageFrame {
    pub(crate) fn of(page: &PdfPage) -> Result<Self, PdfiumError> {
        let visible = page.boundaries().bounding()?.bounds;
        Ok(Self {
            left: visible.left().value,
            bottom: visible.bottom().value,
            width: visible.width().value,
            height: visible.height().value,
            rotation: page.rotation()?,
        })
    }

    fn point(&self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = (x - self.left, y - self.bottom);
        match self.rotation {
            PdfPageRenderRotation::None => (x, self.height - y),
            PdfPageRenderRotation::Degrees90 => (y, x),
            PdfPageRenderRotation::Degrees180 => (self.width - x, y),
            PdfPageRenderRotation::Degrees270 => (self.height - y, self.width - x),
        }
    }

    fn rect(&self, rect: &PdfRect) -> Rect {
        let (x1, y1) = self.point(rect.left().value, rect.bottom().value);
        let (x2, y2) = self.point(rect.right().value, rect.top().value);
        Rect { left: x1.min(x2), top: y1.min(y2), right: x1.max(x2), bottom: y1.max(y2) }
    }

    /// The height on screen of a link target, if the PDF sets the coordinate that gives it.
    fn target_y(&self, x: Option<f32>, y: Option<f32>) -> Option<f32> {
        match self.rotation {
            PdfPageRenderRotation::None | PdfPageRenderRotation::Degrees180 => {
                y.map(|y| self.point(self.left, y).1)
            }
            PdfPageRenderRotation::Degrees90 | PdfPageRenderRotation::Degrees270 => {
                x.map(|x| self.point(x, self.bottom).1)
            }
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

/// `document` is here because link targets can be on any page.
pub(crate) fn extract(page: &PdfPage, document: &PdfDocument) -> Result<PageContent, PdfiumError> {
    let frame = PageFrame::of(page)?;
    let text = page.text()?;
    let chars: Box<[PageChar]> = text
        .chars()
        .iter()
        .filter_map(|character| {
            let rect = character.loose_bounds().map_or(Rect::default(), |rect| frame.rect(&rect));
            Some(PageChar { ch: character.unicode_char()?, rect })
        })
        .collect();
    let links = page
        .links()
        .iter()
        .filter_map(|link| {
            Some(PageLink {
                rect: frame.rect(&link.rect().ok()?),
                target: link_target(&link, document)?,
            })
        })
        .collect();
    Ok(PageContent { chars, links })
}

fn link_target(link: &PdfLink, document: &PdfDocument) -> Option<LinkTarget> {
    if let Some(destination) = link.destination() {
        return page_target(&destination, document);
    }
    match link.action()? {
        PdfAction::LocalDestination(action) => page_target(&action.destination().ok()?, document),
        PdfAction::Uri(action) => Some(LinkTarget::Uri(action.uri().ok()?)),
        _ => None,
    }
}

fn page_target(destination: &PdfDestination, document: &PdfDocument) -> Option<LinkTarget> {
    let index = destination.page_index().ok()?;
    let page = usize::from(index);
    let frame = PageFrame::of(&document.pages().get(index).ok()?).ok()?;
    let (x, y) = match destination.view_settings().ok()? {
        PdfDestinationViewSettings::SpecificCoordinatesAndZoom(x, y, _) => (x, y),
        PdfDestinationViewSettings::FitPageHorizontallyToWindow(y) => (None, y),
        PdfDestinationViewSettings::FitPageVerticallyToWindow(x) => (x, None),
        PdfDestinationViewSettings::FitPageToRectangle(rect) => (Some(rect.left()), Some(rect.top())),
        _ => (None, None),
    };
    Some(LinkTarget::Page {
        page,
        y: frame.target_y(x.map(|x| x.value), y.map(|y| y.value)),
    })
}

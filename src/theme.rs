use cursive::theme::{BaseColor, Color, Palette, PaletteColor, Theme};

pub fn custom_theme() -> Theme {
    let mut palette = Palette::default();

    palette[PaletteColor::Background] = Color::Dark(BaseColor::Black);
    palette[PaletteColor::View] = Color::Dark(BaseColor::Black);
    palette[PaletteColor::Primary] = Color::Dark(BaseColor::White);
    palette[PaletteColor::TitlePrimary] = Color::Dark(BaseColor::Cyan);
    palette[PaletteColor::Highlight] = Color::Dark(BaseColor::Black);
    palette[PaletteColor::HighlightText] = Color::Light(BaseColor::White);
    palette[PaletteColor::Secondary] = Color::Light(BaseColor::White);

    Theme {
        palette,
        ..Theme::default()
    }
}

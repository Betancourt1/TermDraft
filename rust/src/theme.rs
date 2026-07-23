//! Built-in terminal themes applied to the completed Ratatui frame.

use ratatui::buffer::Buffer;
use ratatui::style::Color;

/// The four built-in visual themes available from COMMAND mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Theme {
    Paper,
    Linen,
    Midnight,
    #[default]
    Carbon,
}

impl Theme {
    pub const ALL: [Self; 4] = [Self::Paper, Self::Linen, Self::Midnight, Self::Carbon];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Paper => "Paper",
            Self::Linen => "Linen",
            Self::Midnight => "Midnight",
            Self::Carbon => "Carbon",
        }
    }

    #[must_use]
    pub const fn is_light(self) -> bool {
        matches!(self, Self::Paper | Self::Linen)
    }

    #[must_use]
    pub(crate) const fn terminal_cursor_color(self) -> Option<(u8, u8, u8)> {
        match self {
            Self::Paper => Some((23, 77, 70)),
            Self::Linen => Some((117, 52, 46)),
            Self::Midnight | Self::Carbon => None,
        }
    }

    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Paper => Self::Linen,
            Self::Linen => Self::Midnight,
            Self::Midnight => Self::Carbon,
            Self::Carbon => Self::Paper,
        }
    }

    pub(crate) fn apply(self, buffer: &mut Buffer) {
        let palette = self.palette();
        let area = buffer.area;
        for row in area.top()..area.bottom() {
            for column in area.left()..area.right() {
                let cell = &mut buffer[(column, row)];
                cell.fg = palette.foreground(cell.fg);
                cell.bg = palette.background_color(cell.bg);
            }
        }
    }

    const fn palette(self) -> Palette {
        match self {
            Self::Paper => Palette {
                background: rgb(247, 243, 234),
                popup: rgb(255, 253, 248),
                cursor_line: rgb(241, 236, 226),
                surface: rgb(238, 232, 221),
                panel: rgb(231, 223, 210),
                selection: rgb(207, 222, 216),
                border: rgb(191, 181, 166),
                line_number: rgb(154, 143, 129),
                popup_border: rgb(128, 149, 140),
                faint: rgb(130, 119, 105),
                muted: rgb(102, 92, 81),
                folder: rgb(82, 116, 105),
                file: rgb(42, 110, 101),
                heading_four: rgb(54, 82, 75),
                text: rgb(48, 43, 39),
                heading_three: rgb(36, 81, 73),
                heading_two: rgb(25, 91, 82),
                accent: rgb(23, 77, 70),
                heading_one: rgb(17, 70, 64),
            },
            Self::Linen => Palette {
                background: rgb(251, 247, 237),
                popup: rgb(255, 250, 240),
                cursor_line: rgb(247, 239, 224),
                surface: rgb(244, 234, 215),
                panel: rgb(236, 223, 199),
                selection: rgb(230, 203, 178),
                border: rgb(196, 173, 137),
                line_number: rgb(162, 139, 104),
                popup_border: rgb(178, 133, 99),
                faint: rgb(136, 116, 87),
                muted: rgb(111, 94, 70),
                folder: rgb(144, 93, 63),
                file: rgb(167, 75, 62),
                heading_four: rgb(95, 64, 45),
                text: rgb(62, 52, 38),
                heading_three: rgb(137, 62, 52),
                heading_two: rgb(127, 56, 48),
                accent: rgb(117, 52, 46),
                heading_one: rgb(103, 45, 40),
            },
            Self::Midnight => Palette {
                background: rgb(11, 16, 32),
                popup: rgb(8, 13, 24),
                cursor_line: rgb(14, 22, 41),
                surface: rgb(17, 24, 43),
                panel: rgb(24, 34, 56),
                selection: rgb(40, 61, 92),
                border: rgb(44, 59, 87),
                line_number: rgb(72, 88, 116),
                popup_border: rgb(65, 87, 120),
                faint: rgb(100, 117, 144),
                muted: rgb(130, 145, 169),
                folder: rgb(119, 151, 194),
                file: rgb(130, 183, 219),
                heading_four: rgb(183, 203, 229),
                text: rgb(213, 222, 236),
                heading_three: rgb(188, 214, 244),
                heading_two: rgb(177, 210, 250),
                accent: rgb(169, 204, 255),
                heading_one: rgb(190, 219, 255),
            },
            Self::Carbon => Palette {
                background: Color::Black,
                popup: rgb(6, 6, 6),
                cursor_line: rgb(10, 10, 10),
                surface: rgb(16, 16, 16),
                panel: rgb(28, 28, 28),
                selection: rgb(44, 44, 44),
                border: rgb(58, 58, 58),
                line_number: rgb(72, 72, 72),
                popup_border: rgb(82, 82, 82),
                faint: rgb(92, 92, 92),
                muted: rgb(118, 118, 118),
                folder: rgb(142, 142, 142),
                file: rgb(184, 184, 184),
                heading_four: rgb(196, 196, 196),
                text: rgb(218, 218, 218),
                heading_three: rgb(226, 226, 226),
                heading_two: rgb(232, 232, 232),
                accent: rgb(242, 242, 242),
                heading_one: rgb(246, 246, 246),
            },
        }
    }
}

#[derive(Clone, Copy)]
struct Palette {
    background: Color,
    popup: Color,
    cursor_line: Color,
    surface: Color,
    panel: Color,
    selection: Color,
    border: Color,
    line_number: Color,
    popup_border: Color,
    faint: Color,
    muted: Color,
    folder: Color,
    file: Color,
    heading_four: Color,
    text: Color,
    heading_three: Color,
    heading_two: Color,
    accent: Color,
    heading_one: Color,
}

impl Palette {
    const fn foreground(self, source: Color) -> Color {
        match source {
            Color::Black => self.background,
            Color::Gray | Color::Rgb(242, 242, 242) => self.accent,
            Color::Rgb(6, 6, 6) => self.popup,
            Color::Rgb(10, 10, 10) => self.cursor_line,
            Color::Rgb(16, 16, 16) => self.surface,
            Color::Rgb(28, 28, 28) => self.panel,
            Color::Rgb(42, 42, 42) | Color::Rgb(44, 44, 44) | Color::Rgb(68, 68, 68) => {
                self.selection
            }
            Color::Rgb(58, 58, 58) => self.border,
            Color::Rgb(72, 72, 72) => self.line_number,
            Color::Rgb(82, 82, 82) => self.popup_border,
            Color::Rgb(92, 92, 92) => self.faint,
            Color::Rgb(118, 118, 118) => self.muted,
            Color::Rgb(142, 142, 142) => self.folder,
            Color::Rgb(184, 184, 184) => self.file,
            Color::Rgb(196, 196, 196) => self.heading_four,
            Color::Reset | Color::Rgb(218, 218, 218) => self.text,
            Color::Rgb(226, 226, 226) => self.heading_three,
            Color::Rgb(232, 232, 232) => self.heading_two,
            Color::Rgb(246, 246, 246) => self.heading_one,
            other => other,
        }
    }

    const fn background_color(self, source: Color) -> Color {
        if matches!(source, Color::Reset) {
            self.background
        } else {
            self.foreground(source)
        }
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> Color {
    Color::Rgb(red, green, blue)
}

#[cfg(test)]
mod tests {
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::*;

    #[test]
    fn provides_two_light_and_two_dark_themes() {
        assert_eq!(
            Theme::ALL.iter().filter(|theme| theme.is_light()).count(),
            2
        );
        assert_eq!(
            Theme::ALL.iter().filter(|theme| !theme.is_light()).count(),
            2
        );
    }

    #[test]
    fn light_themes_provide_dark_terminal_cursor_colors() {
        assert_eq!(Theme::Paper.terminal_cursor_color(), Some((23, 77, 70)));
        assert_eq!(Theme::Linen.terminal_cursor_color(), Some((117, 52, 46)));
        assert_eq!(Theme::Midnight.terminal_cursor_color(), None);
        assert_eq!(Theme::Carbon.terminal_cursor_color(), None);
    }

    #[test]
    fn cycles_through_every_theme() {
        let mut theme = Theme::default();
        let mut visited = Vec::new();
        for _ in 0..Theme::ALL.len() {
            theme = theme.next();
            visited.push(theme);
        }

        assert_eq!(visited, Theme::ALL);
        assert_eq!(theme.next(), Theme::Paper);
    }

    #[test]
    fn applies_light_colors_to_foreground_and_background() {
        let mut buffer = Buffer::empty(Rect::new(0, 0, 2, 1));
        buffer[(0, 0)].set_fg(Color::Rgb(218, 218, 218));
        buffer[(0, 0)].set_bg(Color::Black);
        buffer[(1, 0)].set_fg(Color::Rgb(242, 242, 242));
        buffer[(1, 0)].set_bg(Color::Rgb(44, 44, 44));

        Theme::Paper.apply(&mut buffer);

        assert_eq!(buffer[(0, 0)].fg, rgb(48, 43, 39));
        assert_eq!(buffer[(0, 0)].bg, rgb(247, 243, 234));
        assert_eq!(buffer[(1, 0)].fg, rgb(23, 77, 70));
        assert_eq!(buffer[(1, 0)].bg, rgb(207, 222, 216));
    }
}

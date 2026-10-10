//! SGR blink, which `alacritty_terminal` ignores. The parser drives this thin wrapper instead of
//! the terminal itself: every call is passed through, except that blink on and off (SGR 5, 6
//! and 25) set and clear [`BLINK`] in the cursor's template, which the terminal then copies into
//! every cell it writes, as it does its own attributes. SGR 0 clears it with the rest.

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::term::Term;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::vte::ansi::cursor_icon::CursorIcon;
use alacritty_terminal::vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, PrivateMode, Rgb, ScpCharPath, ScpUpdateMode,
    StandardCharset, TabulationClearMode,
};

/// Text the server asked to blink. A flag bit `alacritty_terminal` does not use.
pub const BLINK: Flags = Flags::from_bits_retain(1 << 15);

pub(crate) struct Feeder<'a> {
    pub term: &'a mut Term<VoidListener>,
}

impl Handler for Feeder<'_> {
    fn terminal_attribute(&mut self, attr: Attr) {
        let template = &mut self.term.grid_mut().cursor.template;
        match attr {
            Attr::BlinkSlow | Attr::BlinkFast => template.flags.insert(BLINK),
            Attr::CancelBlink => template.flags.remove(BLINK),
            other => self.term.terminal_attribute(other),
        }
    }

    #[inline]
    fn set_title(&mut self, a0: Option<String>) {
        self.term.set_title(a0);
    }

    #[inline]
    fn set_cursor_style(&mut self, a0: Option<CursorStyle>) {
        self.term.set_cursor_style(a0);
    }

    #[inline]
    fn set_cursor_shape(&mut self, a0: CursorShape) {
        self.term.set_cursor_shape(a0);
    }

    #[inline]
    fn input(&mut self, a0: char) {
        self.term.input(a0);
    }

    #[inline]
    fn goto(&mut self, a0: i32, a1: usize) {
        self.term.goto(a0, a1);
    }

    #[inline]
    fn goto_line(&mut self, a0: i32) {
        self.term.goto_line(a0);
    }

    #[inline]
    fn goto_col(&mut self, a0: usize) {
        self.term.goto_col(a0);
    }

    #[inline]
    fn insert_blank(&mut self, a0: usize) {
        self.term.insert_blank(a0);
    }

    #[inline]
    fn move_up(&mut self, a0: usize) {
        self.term.move_up(a0);
    }

    #[inline]
    fn move_down(&mut self, a0: usize) {
        self.term.move_down(a0);
    }

    #[inline]
    fn identify_terminal(&mut self, a0: Option<char>) {
        self.term.identify_terminal(a0);
    }

    #[inline]
    fn device_status(&mut self, a0: usize) {
        self.term.device_status(a0);
    }

    #[inline]
    fn move_forward(&mut self, a0: usize) {
        self.term.move_forward(a0);
    }

    #[inline]
    fn move_backward(&mut self, a0: usize) {
        self.term.move_backward(a0);
    }

    #[inline]
    fn move_down_and_cr(&mut self, a0: usize) {
        self.term.move_down_and_cr(a0);
    }

    #[inline]
    fn move_up_and_cr(&mut self, a0: usize) {
        self.term.move_up_and_cr(a0);
    }

    #[inline]
    fn put_tab(&mut self, a0: u16) {
        self.term.put_tab(a0);
    }

    #[inline]
    fn backspace(&mut self) {
        self.term.backspace();
    }

    #[inline]
    fn carriage_return(&mut self) {
        self.term.carriage_return();
    }

    #[inline]
    fn linefeed(&mut self) {
        self.term.linefeed();
    }

    #[inline]
    fn bell(&mut self) {
        self.term.bell();
    }

    #[inline]
    fn substitute(&mut self) {
        self.term.substitute();
    }

    #[inline]
    fn newline(&mut self) {
        self.term.newline();
    }

    #[inline]
    fn set_horizontal_tabstop(&mut self) {
        self.term.set_horizontal_tabstop();
    }

    #[inline]
    fn scroll_up(&mut self, a0: usize) {
        self.term.scroll_up(a0);
    }

    #[inline]
    fn scroll_down(&mut self, a0: usize) {
        self.term.scroll_down(a0);
    }

    #[inline]
    fn insert_blank_lines(&mut self, a0: usize) {
        self.term.insert_blank_lines(a0);
    }

    #[inline]
    fn delete_lines(&mut self, a0: usize) {
        self.term.delete_lines(a0);
    }

    #[inline]
    fn erase_chars(&mut self, a0: usize) {
        self.term.erase_chars(a0);
    }

    #[inline]
    fn delete_chars(&mut self, a0: usize) {
        self.term.delete_chars(a0);
    }

    #[inline]
    fn move_backward_tabs(&mut self, a0: u16) {
        self.term.move_backward_tabs(a0);
    }

    #[inline]
    fn move_forward_tabs(&mut self, a0: u16) {
        self.term.move_forward_tabs(a0);
    }

    #[inline]
    fn save_cursor_position(&mut self) {
        self.term.save_cursor_position();
    }

    #[inline]
    fn restore_cursor_position(&mut self) {
        self.term.restore_cursor_position();
    }

    #[inline]
    fn clear_line(&mut self, a0: LineClearMode) {
        self.term.clear_line(a0);
    }

    #[inline]
    fn clear_screen(&mut self, a0: ClearMode) {
        self.term.clear_screen(a0);
    }

    #[inline]
    fn clear_tabs(&mut self, a0: TabulationClearMode) {
        self.term.clear_tabs(a0);
    }

    #[inline]
    fn set_tabs(&mut self, a0: u16) {
        self.term.set_tabs(a0);
    }

    #[inline]
    fn reset_state(&mut self) {
        self.term.reset_state();
    }

    #[inline]
    fn reverse_index(&mut self) {
        self.term.reverse_index();
    }

    #[inline]
    fn set_mode(&mut self, a0: Mode) {
        self.term.set_mode(a0);
    }

    #[inline]
    fn unset_mode(&mut self, a0: Mode) {
        self.term.unset_mode(a0);
    }

    #[inline]
    fn report_mode(&mut self, a0: Mode) {
        self.term.report_mode(a0);
    }

    #[inline]
    fn set_private_mode(&mut self, a0: PrivateMode) {
        self.term.set_private_mode(a0);
    }

    #[inline]
    fn unset_private_mode(&mut self, a0: PrivateMode) {
        self.term.unset_private_mode(a0);
    }

    #[inline]
    fn report_private_mode(&mut self, a0: PrivateMode) {
        self.term.report_private_mode(a0);
    }

    #[inline]
    fn set_scrolling_region(&mut self, a0: usize, a1: Option<usize>) {
        self.term.set_scrolling_region(a0, a1);
    }

    #[inline]
    fn set_keypad_application_mode(&mut self) {
        self.term.set_keypad_application_mode();
    }

    #[inline]
    fn unset_keypad_application_mode(&mut self) {
        self.term.unset_keypad_application_mode();
    }

    #[inline]
    fn set_active_charset(&mut self, a0: CharsetIndex) {
        self.term.set_active_charset(a0);
    }

    #[inline]
    fn configure_charset(&mut self, a0: CharsetIndex, a1: StandardCharset) {
        self.term.configure_charset(a0, a1);
    }

    #[inline]
    fn set_color(&mut self, a0: usize, a1: Rgb) {
        self.term.set_color(a0, a1);
    }

    #[inline]
    fn dynamic_color_sequence(&mut self, a0: String, a1: usize, a2: &str) {
        self.term.dynamic_color_sequence(a0, a1, a2);
    }

    #[inline]
    fn reset_color(&mut self, a0: usize) {
        self.term.reset_color(a0);
    }

    #[inline]
    fn clipboard_store(&mut self, a0: u8, a1: &[u8]) {
        self.term.clipboard_store(a0, a1);
    }

    #[inline]
    fn clipboard_load(&mut self, a0: u8, a1: &str) {
        self.term.clipboard_load(a0, a1);
    }

    #[inline]
    fn decaln(&mut self) {
        self.term.decaln();
    }

    #[inline]
    fn push_title(&mut self) {
        self.term.push_title();
    }

    #[inline]
    fn pop_title(&mut self) {
        self.term.pop_title();
    }

    #[inline]
    fn text_area_size_pixels(&mut self) {
        self.term.text_area_size_pixels();
    }

    #[inline]
    fn text_area_size_chars(&mut self) {
        self.term.text_area_size_chars();
    }

    #[inline]
    fn set_hyperlink(&mut self, a0: Option<Hyperlink>) {
        self.term.set_hyperlink(a0);
    }

    #[inline]
    fn set_mouse_cursor_icon(&mut self, a0: CursorIcon) {
        self.term.set_mouse_cursor_icon(a0);
    }

    #[inline]
    fn report_keyboard_mode(&mut self) {
        self.term.report_keyboard_mode();
    }

    #[inline]
    fn push_keyboard_mode(&mut self, a0: KeyboardModes) {
        self.term.push_keyboard_mode(a0);
    }

    #[inline]
    fn pop_keyboard_modes(&mut self, a0: u16) {
        self.term.pop_keyboard_modes(a0);
    }

    #[inline]
    fn set_keyboard_mode(&mut self, a0: KeyboardModes, a1: KeyboardModesApplyBehavior) {
        self.term.set_keyboard_mode(a0, a1);
    }

    #[inline]
    fn set_modify_other_keys(&mut self, a0: ModifyOtherKeys) {
        self.term.set_modify_other_keys(a0);
    }

    #[inline]
    fn report_modify_other_keys(&mut self) {
        self.term.report_modify_other_keys();
    }

    #[inline]
    fn set_scp(&mut self, a0: ScpCharPath, a1: ScpUpdateMode) {
        self.term.set_scp(a0, a1);
    }
}

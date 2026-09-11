//! Where a list of rows stands: the cursor, the first row on screen, and how many fit.

#[derive(Default)]
pub struct List {
    pub cursor: usize,
    pub scroll: usize,
    /// Rows the last render had room for; what paging and scrolling are measured against.
    pub page: usize,
    len: usize,
}

impl List {
    /// Tells the list how many rows it now has; the cursor and the scroll are kept within.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
        self.clamp();
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page.max(1);
        self.clamp();
    }

    pub fn clamp(&mut self) {
        if self.len == 0 {
            self.cursor = 0;
            self.scroll = 0;
            return;
        }
        let page = self.page.max(1);
        self.cursor = self.cursor.min(self.len - 1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        }
        if self.cursor >= self.scroll + page {
            self.scroll = self.cursor + 1 - page;
        }
        let max_scroll = self.len.saturating_sub(page);
        self.scroll = self.scroll.min(max_scroll);
    }

    pub fn move_by(&mut self, delta: isize) {
        let target = self.cursor as isize + delta;
        self.cursor = target.max(0) as usize;
        self.clamp();
    }

    pub fn half_page(&self) -> isize {
        (self.page / 2).max(1) as isize
    }

    pub fn select(&mut self, index: usize) {
        self.cursor = index;
        self.clamp();
    }

    pub fn home(&mut self) {
        self.select(0);
    }

    pub fn end(&mut self) {
        self.select(self.len.saturating_sub(1));
    }

    /// Scrolls the rows under a cursor that stays on screen.
    pub fn scroll_by(&mut self, delta: isize) {
        let page = self.page.max(1);
        let max_scroll = self.len.saturating_sub(page) as isize;
        let target = (self.scroll as isize + delta).clamp(0, max_scroll);
        self.scroll = target as usize;
        if self.cursor < self.scroll {
            self.cursor = self.scroll;
        }
        let last_visible = self.scroll + page - 1;
        if self.cursor > last_visible {
            self.cursor = last_visible;
        }
        self.clamp();
    }

    /// Puts the cursor's row in the middle of the screen.
    pub fn center(&mut self) {
        self.scroll = self.cursor.saturating_sub(self.page / 2);
        self.clamp();
    }

    /// Whether the cursor is within a screen of the last row.
    pub fn near_end(&self) -> bool {
        self.cursor + self.page >= self.len
    }

    /// The row a screen line lands on, if any.
    pub fn row_at(&self, line: usize) -> Option<usize> {
        let index = self.scroll + line;
        (index < self.len).then_some(index)
    }
}

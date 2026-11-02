// The cells of the web build's test: the same in the page and on its server.
#[aio::cell(shared)]
pub struct Board {
    pub items: Vec<String>,
}

#[aio::methods]
impl Board {
    #[client]
    pub fn add(&mut self, text: String) -> aio::Result {
        self.items.push(text);
        Ok(())
    }
}

// Another synced cell: on the page's one socket with the board.
#[aio::cell(shared)]
pub struct Votes {
    pub n: u32,
}

#[aio::methods]
impl Votes {
    #[client]
    pub fn vote(&mut self) -> aio::Result {
        self.n += 1;
        Ok(())
    }
}

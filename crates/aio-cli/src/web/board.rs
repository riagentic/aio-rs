// The cell of the web build's test: the same in the page and on its server.
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

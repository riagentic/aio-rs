// A cell of the page itself: kept in the browser, no server.
#[aio::cell(persist)]
pub struct Tally {
    pub n: u32,
    #[transient]
    pub busy: bool,
}

/// I/O in the page: a wait on its timer.
async fn tick() -> u32 {
    aio::web::sleep(200).await;
    10
}

#[aio::methods]
impl Tally {
    pub fn bump(&mut self) -> aio::Result {
        aio::ensure!(self.n < 3, "full");
        self.n += 1;
        self.busy = true;
        Ok(())
    }

    /// An async method: its effect runs on the page's tasks.
    pub async fn later(&mut self) -> aio::Result {
        let got: u32 = tick().await;
        self.n += got;
        Ok(())
    }
}

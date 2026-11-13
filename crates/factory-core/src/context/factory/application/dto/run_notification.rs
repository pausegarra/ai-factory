/// An immutable, safe projection of one committed run transition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunNotification {
    pub run_id: String,
    pub attempt: u32,
    pub status: String,
    pub title: String,
    pub body: String,
}

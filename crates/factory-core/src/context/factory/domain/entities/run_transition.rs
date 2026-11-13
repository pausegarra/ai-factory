#[derive(Clone, Debug)]
pub(crate) struct RunTransition {
    pub previous_status: String,
    pub run: crate::Run,
}

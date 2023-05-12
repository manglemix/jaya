/// A component, for now, is simply a thread safe object
pub trait Component: Send + Sync { }

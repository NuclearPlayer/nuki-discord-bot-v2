/// The thread that a message of the history is connected to.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadMarker {
    /// The message started a thread, and its conversation continues there.
    Started { name: String, reply_count: u32 },
    /// The message is the first message of the history of a thread.
    Opens {
        name: String,
        parent_channel_name: String,
        reply_count: u32,
        /// The message started the thread. Else the history does not contain the starter
        /// message, for example because it was deleted.
        has_starter: bool,
    },
}

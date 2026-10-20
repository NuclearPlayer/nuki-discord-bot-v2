/// The thread that a history message is connected to.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadMarker {
    /// The message started a thread, and its conversation continues there.
    Started { name: String, reply_count: u32 },
    /// The message is the first message of a thread's history.
    Opens {
        name: String,
        parent_channel_name: String,
        reply_count: u32,
        /// If `true`, the message started the thread. If `false`, the history does not contain
        /// the starter message, for example because the starter message was deleted.
        has_starter: bool,
    },
}

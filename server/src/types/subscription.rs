use opcua_core::types::*;

use types::monitored_item::*;

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SubscriptionState {
    Closed,
    Creating,
    Normal,
    Late,
    KeepAlive
}

#[derive(Debug, Clone, PartialEq)]
pub struct Subscription {
    pub subscription_id: UInt32,
    /// Flag enabling publishing
    pub publishing_enabled: Boolean,
    /// State of the subscription
    pub state: SubscriptionState,
    /// Publishing interval
    pub publishing_interval: Double,
    /// Lifetime count enforced
    pub lifetime_count: UInt32,
    /// Keep alive count enforced
    pub keep_alive_count: UInt32,
    /// Relative priority of the subscription. When more than
    /// one subscription needs to send notifications the highest
    /// priority subscription should be sent first.
    pub priority: Byte,
    /// List of monitored items
    pub monitored_items: Vec<MonitoredItem>,
}


impl Subscription {
    pub fn delete_acknowledged_notification_messages() {}

    pub fn enqueue_pushing_request() {}

    pub fn reset_lifetime_counter() {}

    pub fn return_notifications() {}

    pub fn start_publishing_timer() {}

    pub fn dequeue_pubish_request() {}

    pub fn return_keep_alive() {}

    pub fn reset_keep_alive_counter() {}
}
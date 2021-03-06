use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use chrono;

use opcua_types::*;
use opcua_types::status_code::StatusCode;
use opcua_types::service_types::{TimestampsToReturn, NotificationMessage, MonitoredItemCreateRequest, MonitoredItemCreateResult, MonitoredItemModifyRequest, MonitoredItemModifyResult};

use crate::{
    constants,
    DateTimeUtc,
    subscriptions::monitored_item::MonitoredItem,
    address_space::AddressSpace,
    diagnostics::ServerDiagnostics,
};

/// The state of the subscription
#[derive(Debug, Copy, Clone, PartialEq, Serialize)]
pub enum SubscriptionState {
    Closed,
    Creating,
    Normal,
    Late,
    KeepAlive,
}

#[derive(Debug)]
pub struct SubscriptionStateParams {
    pub notifications_available: bool,
    pub more_notifications: bool,
    pub publishing_req_queued: bool,
    pub publishing_interval_elapsed: bool,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum UpdateStateAction {
    None,
    ReturnKeepAlive,
    ReturnNotifications,
}

/// This is for debugging purposes. It allows the caller to validate the output state if required.
///
/// Values correspond to state table in OPC UA Part 4 5.13.1.2
///
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum HandledState {
    None0 = 0,
    Closed1 = 1,
    // Create2 = 2,
    Create3 = 3,
    Normal4 = 4,
    Normal5 = 5,
    IntervalElapsed6 = 6,
    IntervalElapsed7 = 7,
    IntervalElapsed8 = 8,
    IntervalElapsed9 = 9,
    Late10 = 10,
    Late11 = 11,
    Late12 = 12,
    KeepAlive13 = 13,
    KeepAlive14 = 14,
    KeepAlive15 = 15,
    KeepAlive16 = 16,
    KeepAlive17 = 17,

}

/// This is for debugging purposes. It allows the caller to validate the output state if required.
#[derive(Debug)]
pub struct UpdateStateResult {
    pub handled_state: HandledState,
    pub update_state_action: UpdateStateAction,
}

impl UpdateStateResult {
    pub fn new(handled_state: HandledState, update_state_action: UpdateStateAction) -> UpdateStateResult {
        UpdateStateResult {
            handled_state,
            update_state_action,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TickReason {
    ReceivedPublishRequest,
    TickTimerFired,
}

#[derive(Debug, Clone, Serialize)]
pub struct Subscription {
    /// Subscription id
    pub subscription_id: u32,
    /// Publishing interval in milliseconds
    pub publishing_interval: Duration,
    /// The lifetime count reset value
    pub max_lifetime_count: u32,
    /// Keep alive count reset value
    pub max_keep_alive_count: u32,
    /// Relative priority of the subscription. When more than one subscriptio
    ///  needs to send notifications the highest priority subscription should
    /// be sent first.
    pub priority: u8,
    /// Map of monitored items
    pub monitored_items: HashMap<u32, MonitoredItem>,
    /// State of the subscription
    pub state: SubscriptionState,
    /// A value that contains the number of consecutive publishing timer expirations without Client
    /// activity before the Subscription is terminated.
    pub current_lifetime_count: u32,
    /// Keep alive counter decrements when there are no notifications to publish and when it expires
    /// requests to send an empty notification as a keep alive event
    pub current_keep_alive_count: u32,
    /// boolean value that is set to true to mean that either a NotificationMessage or a keep-alive
    /// Message has been sent on the Subscription. It is a flag that is used to ensure that either
    /// a NotificationMessage or a keep-alive Message is sent out the first time the publishing timer
    /// expires.
    pub message_sent: bool,
    /// The parameter that requests publishing to be enabled or disabled.
    pub publishing_enabled: bool,
    /// A flag that tells the subscription to send the latest value of every monitored item on the
    /// next publish request.
    resend_data: bool,
    /// The next sequence number to be sent
    next_sequence_number: u32,
    // The last monitored item id
    next_monitored_item_id: u32,
    // The time that the subscription interval last fired
    last_timer_expired_time: DateTimeUtc,
    /// Server diagnostics to track creation / destruction / modification of the subscription
    #[serde(skip)]
    diagnostics: Arc<RwLock<ServerDiagnostics>>,
    /// Stops the subscription calling diagnostics on drop
    #[serde(skip)]
    pub diagnostics_on_drop: bool,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if self.diagnostics_on_drop {
            let mut diagnostics = trace_write_lock_unwrap!(self.diagnostics);
            diagnostics.on_destroy_subscription(self);
        }
    }
}

impl Subscription {
    pub fn new(diagnostics: Arc<RwLock<ServerDiagnostics>>, subscription_id: u32, publishing_enabled: bool, publishing_interval: Duration, lifetime_count: u32, keep_alive_count: u32, priority: u8) -> Subscription {
        let subscription = Subscription {
            subscription_id,
            publishing_interval,
            priority,
            monitored_items: HashMap::with_capacity(constants::DEFAULT_MONITORED_ITEM_CAPACITY),
            max_lifetime_count: lifetime_count,
            max_keep_alive_count: keep_alive_count,
            // State variables
            state: SubscriptionState::Creating,
            current_lifetime_count: lifetime_count,
            current_keep_alive_count: keep_alive_count,
            message_sent: false,
            publishing_enabled,
            resend_data: false,
            // Counters for new items
            next_sequence_number: 1,
            next_monitored_item_id: 1,
            last_timer_expired_time: chrono::Utc::now(),
            diagnostics,
            diagnostics_on_drop: true,
        };
        {
            let mut diagnostics = trace_write_lock_unwrap!(subscription.diagnostics);
            diagnostics.on_create_subscription(&subscription);
        }
        subscription
    }

    /// Creates monitored items on the specified subscription, returning the creation results
    pub fn create_monitored_items(&mut self, timestamps_to_return: TimestampsToReturn, items_to_create: &[MonitoredItemCreateRequest]) -> Vec<MonitoredItemCreateResult> {
        self.reset_lifetime_counter();

        // Add items to the subscription if they're not already in its
        items_to_create.iter().map(|item_to_create| {
            // Create a monitored item, if possible
            let monitored_item_id = self.next_monitored_item_id;
            match MonitoredItem::new(monitored_item_id, timestamps_to_return, item_to_create) {
                Ok(monitored_item) => {
                    // Register the item with the subscription
                    let revised_sampling_interval = monitored_item.sampling_interval;
                    let revised_queue_size = monitored_item.queue_size as u32;
                    self.monitored_items.insert(monitored_item_id, monitored_item);
                    self.next_monitored_item_id += 1;
                    MonitoredItemCreateResult {
                        status_code: StatusCode::Good,
                        monitored_item_id,
                        revised_sampling_interval,
                        revised_queue_size,
                        filter_result: ExtensionObject::null(),
                    }
                }
                Err(status_code) => {
                    MonitoredItemCreateResult {
                        status_code,
                        monitored_item_id: 0,
                        revised_sampling_interval: 0f64,
                        revised_queue_size: 0,
                        filter_result: ExtensionObject::null(),
                    }
                }
            }
        }).collect()
    }

    /// Modify the specified monitored items, returning a result for each
    pub fn modify_monitored_items(&mut self, timestamps_to_return: TimestampsToReturn, items_to_modify: &[MonitoredItemModifyRequest]) -> Vec<MonitoredItemModifyResult> {
        self.reset_lifetime_counter();
        items_to_modify.iter().map(|item_to_modify| {
            match self.monitored_items.get_mut(&item_to_modify.monitored_item_id) {
                Some(monitored_item) => {
                    // Try to change the monitored item according to the modify request
                    let modify_result = monitored_item.modify(timestamps_to_return, item_to_modify);
                    match modify_result {
                        Ok(filter_result) => MonitoredItemModifyResult {
                            status_code: StatusCode::Good,
                            revised_sampling_interval: monitored_item.sampling_interval,
                            revised_queue_size: monitored_item.queue_size as u32,
                            filter_result,
                        },
                        Err(err) => MonitoredItemModifyResult {
                            status_code: err,
                            revised_sampling_interval: 0f64,
                            revised_queue_size: 0,
                            filter_result: ExtensionObject::null(),
                        }
                    }
                }
                // Item does not exist
                None => MonitoredItemModifyResult {
                    status_code: StatusCode::BadMonitoredItemIdInvalid,
                    revised_sampling_interval: 0f64,
                    revised_queue_size: 0,
                    filter_result: ExtensionObject::null(),
                }
            }
        }).collect()
    }

    /// Delete the specified monitored items (by item id), returning a status code for each
    pub fn delete_monitored_items(&mut self, items_to_delete: &[u32]) -> Vec<StatusCode> {
        self.reset_lifetime_counter();
        items_to_delete.iter().map(|item_to_delete| {
            match self.monitored_items.remove(item_to_delete) {
                Some(_) => StatusCode::Good,
                None => StatusCode::BadMonitoredItemIdInvalid
            }
        }).collect()
    }

    // Returns two vecs representing the server and client handles for each monitored item.
    // Called from the GetMonitoredItems impl
    pub fn get_handles(&self) -> (Vec<u32>, Vec<u32>) {
        let server_handles = self.monitored_items.values().map(|i| i.monitored_item_id).collect();
        let client_handles = self.monitored_items.values().map(|i| i.client_handle).collect();
        (server_handles, client_handles)
    }

    /// Sets the resend data flag which means the next publish request will receive the latest value
    /// of every monitored item whether it has changed in this cycle or not.
    pub fn set_resend_data(&mut self) {
        self.resend_data = true;
    }

    /// Checks the subscription and monitored items for state change, messages. If the tick does
    /// nothing, the function returns None. Otherwise it returns one or more messages in an Vec.
    pub fn tick(&mut self, address_space: &AddressSpace, tick_reason: TickReason, publishing_req_queued: bool, now: &DateTimeUtc) -> Option<NotificationMessage> {
        // Check if the publishing interval has elapsed. Only checks on the tick timer.
        let publishing_interval_elapsed = match tick_reason {
            TickReason::ReceivedPublishRequest => false,
            TickReason::TickTimerFired => if self.state == SubscriptionState::Creating {
                true
            } else if self.publishing_interval <= 0f64 {
                panic!("Publishing interval should have been revised to min interval")
            } else {
                // Look at the last expiration time compared to now and see if it matches
                // or exceeds the publishing interval
                let publishing_interval = super::duration_from_ms(self.publishing_interval);
                if now.signed_duration_since(self.last_timer_expired_time) >= publishing_interval {
                    self.last_timer_expired_time = *now;
                    true
                } else {
                    false
                }
            }
        };

        // Do a tick on monitored items. Note that monitored items normally update when the interval
        // elapses but they don't have to. So this is called every tick just to catch items with their
        // own intervals.
        let (notification_message, more_notifications) = match self.state {
            SubscriptionState::Closed | SubscriptionState::Creating => (None, false),
            _ => {
                let resend_data = self.resend_data;
                self.tick_monitored_items(address_space, now, publishing_interval_elapsed, resend_data)
            }
        };
        self.resend_data = false;
        let notifications_available = notification_message.is_some();

        // If items have changed or subscription interval elapsed then we may have notifications
        // to send or state to update
        let result = if notifications_available || publishing_interval_elapsed || publishing_req_queued {
            // Update the internal state of the subscription based on what happened
            let update_state_result = self.update_state(tick_reason, SubscriptionStateParams {
                publishing_req_queued,
                notifications_available,
                more_notifications,
                publishing_interval_elapsed,
            });
            trace!("subscription tick - update_state_result = {:?}", update_state_result);

            // Now act on the state's action
            match update_state_result.update_state_action {
                UpdateStateAction::None => {
                    if notifications_available {
                        // Reset the next sequence number to the discarded notification
                        let notification_sequence_number = notification_message.unwrap().sequence_number;
                        self.next_sequence_number = notification_sequence_number;
                        debug!("Notification message nr {} was being ignored for a do-nothing, update state was {:?}", notification_sequence_number, update_state_result);
                    }
                    // Send nothing
                    //println!("do nothing {:?}", update_state_result.handled_state);
                    None
                }
                UpdateStateAction::ReturnKeepAlive => {
                    if notifications_available {
                        // Reset the next sequence number to the discarded notification
                        let notification_sequence_number = notification_message.unwrap().sequence_number;
                        self.next_sequence_number = notification_sequence_number;
                        debug!("Notification message nr {} was being ignored for a keep alive, update state was {:?}", notification_sequence_number, update_state_result);
                    }
                    // Send a keep alive
                    debug!("Sending keep alive response");
                    Some(NotificationMessage::keep_alive(self.next_sequence_number, DateTime::from(now.clone())))
                }
                UpdateStateAction::ReturnNotifications => {
                    // Send the notification message
                    debug!("Sending notification response");
                    notification_message
                }
            }
        } else {
            None
        };

        // Check if the subscription interval has been exceeded since last call
        if self.current_lifetime_count == 1 {
            info!("Subscription {} has expired and will be removed shortly", self.subscription_id);
            self.state = SubscriptionState::Closed;
        }

        result
    }

    /// Iterate through the monitored items belonging to the subscription, calling tick on each in turn.
    /// The function returns notifications and a more_notifications boolean.
    fn tick_monitored_items(&mut self, address_space: &AddressSpace, now: &DateTimeUtc, publishing_interval_elapsed: bool, resend_data: bool) -> (Option<NotificationMessage>, bool) {
        let mut notification_messages = Vec::new();
        for (_, monitored_item) in &mut self.monitored_items {
            // If this returns true then the monitored item wants to report its notification
            let _ = monitored_item.tick(address_space, now, publishing_interval_elapsed, resend_data);
            if publishing_interval_elapsed {
                // Take some / all of the monitored item's pending notifications
                if let Some(mut item_notification_messages) = monitored_item.all_notification_messages() {
                    notification_messages.append(&mut item_notification_messages);
                }
            }
        }

        if !notification_messages.is_empty() {
            use std;
            debug!("Create notification for subscription {}, sequence number {}", self.subscription_id, self.next_sequence_number);
            // Create a notification message and push it onto the queue
            let notification = NotificationMessage::data_change(self.next_sequence_number, DateTime::now(), notification_messages);
            // Advance next sequence number
            self.next_sequence_number = if self.next_sequence_number == std::u32::MAX {
                1
            } else {
                self.next_sequence_number + 1
            };
            (Some(notification), false)
        } else {
            (None, false)
        }
    }

    // See OPC UA Part 4 5.13.1.2 State Table
    //
    // This function implements the main guts of updating the subscription's state according to
    // some input events and its existing internal state.
    //
    // Calls to the function will update the internal state of and return a tuple with any required
    // actions.
    //
    // Note that some state events are handled outside of update_state. e.g. the subscription
    // is created elsewhere which handles states 1, 2 and 3.
    //
    // Inputs:
    //
    // * publish_request - an optional publish request. May be used by subscription to remove acknowledged notifications
    // * publishing_interval_elapsed - true if the publishing interval has elapsed
    //
    // Returns in order:
    //
    // * State id that handled this call. Useful for debugging which state handler triggered
    // * Update state action - none, return notifications, return keep alive
    // * Publishing request action - nothing, dequeue
    //
    pub fn update_state(&mut self, tick_reason: TickReason, p: SubscriptionStateParams) -> UpdateStateResult {
        // This function is called when a publish request is received OR the timer expired, so getting
        // both is invalid code somewhere
        if tick_reason == TickReason::ReceivedPublishRequest && p.publishing_interval_elapsed {
            panic!("Should not be possible for timer to have expired and received publish request at same time")
        }

        // Extra state debugging
        {
            use log::Level::Trace;
            if log_enabled!(Trace) {
                trace!(r#"State inputs:
    subscription_id: {} / state: {:?}
    tick_reason: {:?} / state_params: {:?}
    publishing_enabled: {}
    keep_alive_counter / lifetime_counter: {} / {}
    message_sent: {}"#,
                       self.subscription_id, self.state, tick_reason, p,
                       self.publishing_enabled,
                       self.current_keep_alive_count,
                       self.current_lifetime_count,
                       self.message_sent);
            }
        }

        // This is a state engine derived from OPC UA Part 4 Publish service and might look a
        // little odd for that.
        //
        // Note in some cases, some of the actions have already happened outside of this function.
        // For example, publish requests are already queued before we come in here and this function
        // uses what its given. Likewise, this function does not "send" notifications, rather
        // it returns them (if any) and it is up to the caller to send them

        match self.state {
            SubscriptionState::Closed => {
                // State #1
                // TODO
                // if receive_create_subscription {
                // self.state = Subscription::Creating;
                // }
                return UpdateStateResult::new(HandledState::Closed1, UpdateStateAction::None);
            }
            SubscriptionState::Creating => {
                // State #2
                // CreateSubscription fails, return negative response
                // Handled in message handler
                // State #3
                self.state = SubscriptionState::Normal;
                self.message_sent = false;
                return UpdateStateResult::new(HandledState::Create3, UpdateStateAction::None);
            }
            SubscriptionState::Normal => {
                if tick_reason == TickReason::ReceivedPublishRequest {
                    if !self.publishing_enabled || (self.publishing_enabled && !p.more_notifications) {
                        // State #4
                        return UpdateStateResult::new(HandledState::Normal4, UpdateStateAction::None);
                    } else if self.publishing_enabled && p.more_notifications {
                        // State #5
                        self.reset_lifetime_counter();
                        self.message_sent = true;
                        return UpdateStateResult::new(HandledState::Normal5, UpdateStateAction::ReturnNotifications);
                    }
                } else if p.publishing_interval_elapsed {
                    if p.publishing_req_queued && self.publishing_enabled && p.notifications_available {
                        // State #6
                        self.reset_lifetime_counter();
                        self.start_publishing_timer();
                        self.message_sent = true;
                        return UpdateStateResult::new(HandledState::IntervalElapsed6, UpdateStateAction::ReturnNotifications);
                    } else if p.publishing_req_queued && !self.message_sent && (!self.publishing_enabled || (self.publishing_enabled && !p.notifications_available)) {
                        // State #7
                        self.reset_lifetime_counter();
                        self.start_publishing_timer();
                        self.message_sent = true;
                        return UpdateStateResult::new(HandledState::IntervalElapsed7, UpdateStateAction::ReturnKeepAlive);
                    } else if !p.publishing_req_queued && (!self.message_sent || (self.publishing_enabled && p.notifications_available)) {
                        // State #8
                        self.start_publishing_timer();
                        self.state = SubscriptionState::Late;
                        return UpdateStateResult::new(HandledState::IntervalElapsed8, UpdateStateAction::None);
                    } else if self.message_sent && (!self.publishing_enabled || (self.publishing_enabled && !p.notifications_available)) {
                        // State #9
                        self.start_publishing_timer();
                        self.reset_keep_alive_counter();
                        self.state = SubscriptionState::KeepAlive;
                        return UpdateStateResult::new(HandledState::IntervalElapsed9, UpdateStateAction::None);
                    }
                }
            }
            SubscriptionState::Late => {
                if tick_reason == TickReason::ReceivedPublishRequest {
                    if self.publishing_enabled && (p.notifications_available || p.more_notifications) {
                        // State #10
                        self.reset_lifetime_counter();
                        self.state = SubscriptionState::Normal;
                        self.message_sent = true;
                        return UpdateStateResult::new(HandledState::Late10, UpdateStateAction::ReturnNotifications);
                    } else if !self.publishing_enabled || (self.publishing_enabled && !p.notifications_available && !p.more_notifications) {
                        // State #11
                        self.reset_lifetime_counter();
                        self.state = SubscriptionState::KeepAlive;
                        self.message_sent = true;
                        return UpdateStateResult::new(HandledState::Late11, UpdateStateAction::ReturnKeepAlive);
                    }
                } else if p.publishing_interval_elapsed {
                    // State #12
                    self.start_publishing_timer();
                    return UpdateStateResult::new(HandledState::Late12, UpdateStateAction::None);
                }
            }
            SubscriptionState::KeepAlive => {
                if tick_reason == TickReason::ReceivedPublishRequest {
                    // State #13
                    return UpdateStateResult::new(HandledState::KeepAlive13, UpdateStateAction::None);
                } else if p.publishing_interval_elapsed {
                    if self.publishing_enabled && p.notifications_available && p.publishing_req_queued {
                        // State #14
                        self.message_sent = true;
                        self.state = SubscriptionState::Normal;
                        return UpdateStateResult::new(HandledState::KeepAlive14, UpdateStateAction::ReturnNotifications);
                    } else if p.publishing_req_queued && self.current_keep_alive_count == 1 && (!self.publishing_enabled || (self.publishing_enabled && p.notifications_available)) {
                        // State #15
                        self.start_publishing_timer();
                        self.reset_keep_alive_counter();
                        return UpdateStateResult::new(HandledState::KeepAlive15, UpdateStateAction::ReturnKeepAlive);
                    } else if self.current_keep_alive_count > 1 && (!self.publishing_enabled || (self.publishing_enabled && !p.notifications_available)) {
                        // State #16
                        self.start_publishing_timer();
                        self.current_keep_alive_count -= 1;
                        return UpdateStateResult::new(HandledState::KeepAlive16, UpdateStateAction::None);
                    } else if !p.publishing_req_queued && (self.current_keep_alive_count == 1 || (self.current_keep_alive_count > 1 && self.publishing_enabled && p.notifications_available)) {
                        // State #17
                        self.start_publishing_timer();
                        self.state = SubscriptionState::Late;
                        return UpdateStateResult::new(HandledState::KeepAlive17, UpdateStateAction::None);
                    }
                }
            }
        }

        // Some more state tests that match on more than one state
        match self.state {
            SubscriptionState::Normal | SubscriptionState::Late | SubscriptionState::KeepAlive => {
                if self.current_lifetime_count == 1 {
                    // State #27
                    // TODO
                    // delete monitored items
                    // issue_status_change_notification
                }
            }
            _ => {
                // DO NOTHING
            }
        }

        // println!("No state handled {:?}, {:?}", tick_reason, p);
        UpdateStateResult::new(HandledState::None0, UpdateStateAction::None)
    }


    /// Reset the keep-alive counter to the maximum keep-alive count of the Subscription.
    /// The maximum keep-alive count is set by the Client when the Subscription is created
    /// and may be modified using the ModifySubscription Service
    pub fn reset_keep_alive_counter(&mut self) {
        self.current_keep_alive_count = self.max_keep_alive_count;
    }

    /// Reset the lifetime counter to the value specified for the life time of the subscription
    /// in the create subscription service
    pub fn reset_lifetime_counter(&mut self) {
        self.current_lifetime_count = self.max_lifetime_count;
    }

    /// Start or restart the publishing timer and decrement the LifetimeCounter Variable.
    pub fn start_publishing_timer(&mut self) {
        self.current_lifetime_count -= 1;
    }
}
use std::collections::VecDeque;
use std::sync::{Arc, RwLock, Mutex};

use chrono;

use opcua_core::comms::secure_channel::{Role, SecureChannel};
use opcua_core::crypto::X509;
use opcua_types::*;
use opcua_types::service_types::PublishRequest;
use opcua_types::status_code::StatusCode;

use crate::{
    address_space::AddressSpace,
    continuation_point::BrowseContinuationPoint,
    diagnostics::ServerDiagnostics,
    DateTimeUtc,
    server::Server,
    subscriptions::subscription::TickReason,
    subscriptions::subscriptions::Subscriptions,
};

/// Session info holds information about a session created by CreateSession service
#[derive(Clone)]
pub struct SessionInfo {}

const PUBLISH_REQUEST_TIMEOUT: i64 = 30000;

lazy_static! {
    // TODO this should be done with AtomicI32 when it stops being experimental
    static ref LAST_SESSION_ID: Arc<Mutex<i32>> = Arc::new(Mutex::new(0));
}

fn next_session_id() -> NodeId {
    let mut last_session_id = trace_lock_unwrap!(LAST_SESSION_ID);
    *last_session_id += 1;
    NodeId::new(1, *last_session_id)
}

/// The Session is any state maintained between the client and server
pub struct Session {
    /// Subscriptions associated with the session
    pub subscriptions: Subscriptions,
    /// The session identifier
    pub session_id: NodeId,
    /// Flag to indicate session should be terminated
    pub terminate_session: bool,
    /// Security policy
    pub security_policy_uri: String,
    /// Client's certificate
    pub client_certificate: Option<X509>,
    /// Authentication token for the session
    pub authentication_token: NodeId,
    /// Secure channel state
    pub secure_channel: Arc<RwLock<SecureChannel>>,
    /// Session nonce
    pub session_nonce: ByteString,
    /// Session timeout
    pub session_timeout: f64,
    /// User identity token
    pub user_identity: Option<ExtensionObject>,
    /// Negotiated max request message size
    pub max_request_message_size: u32,
    /// Negotiated max response message size
    pub max_response_message_size: u32,
    /// Endpoint url for this session
    pub endpoint_url: UAString,
    /// Maximum number of continuation points
    max_browse_continuation_points: usize,
    /// Browse continuation points (oldest to newest)
    browse_continuation_points: VecDeque<BrowseContinuationPoint>,
    /// Diagnostics associated with the session
    diagnostics: Arc<RwLock<ServerDiagnostics>>,
    /// Indicates if the session has received an ActivateSession
    pub activated: bool,
    /// Time that session was terminated, helps with recovering sessions, or clearing them out
    terminated_at: DateTimeUtc,
    /// Flag indicating session is actually terminated
    terminated: bool,
}

impl Drop for Session {
    fn drop(&mut self) {
        info!("Session is being dropped");
        let mut diagnostics = trace_write_lock_unwrap!(self.diagnostics);
        diagnostics.on_destroy_session(self);
    }
}

impl Session {
    #[cfg(test)]
    pub fn new_no_certificate_store(secure_channel: SecureChannel) -> Session {
        let max_browse_continuation_points = super::constants::MAX_BROWSE_CONTINUATION_POINTS;
        let session = Session {
            subscriptions: Subscriptions::new(100, PUBLISH_REQUEST_TIMEOUT),
            session_id: next_session_id(),
            activated: false,
            terminate_session: false,
            terminated: false,
            terminated_at: chrono::Utc::now(),
            client_certificate: None,
            security_policy_uri: String::new(),
            authentication_token: NodeId::null(),
            secure_channel: Arc::new(RwLock::new(secure_channel)),
            session_nonce: ByteString::null(),
            session_timeout: 0f64,
            user_identity: None,
            max_request_message_size: 0,
            max_response_message_size: 0,
            endpoint_url: UAString::null(),
            max_browse_continuation_points,
            browse_continuation_points: VecDeque::with_capacity(max_browse_continuation_points),
            diagnostics: Arc::new(RwLock::new(ServerDiagnostics::default())),
        };
        {
            let mut diagnostics = trace_write_lock_unwrap!(session.diagnostics);
            diagnostics.on_create_session(&session);
        }
        session
    }

    pub fn new(server: &Server) -> Session {
        let max_browse_continuation_points = super::constants::MAX_BROWSE_CONTINUATION_POINTS;

        let server_state = server.server_state();
        let server_state = trace_read_lock_unwrap!(server_state);
        let max_subscriptions = server_state.max_subscriptions;
        let diagnostics = server_state.diagnostics.clone();
        let decoding_limits = {
            let config = trace_read_lock_unwrap!(server_state.config);
            config.decoding_limits()
        };

        let session = Session {
            subscriptions: Subscriptions::new(max_subscriptions, PUBLISH_REQUEST_TIMEOUT),
            session_id: next_session_id(),
            activated: false,
            terminate_session: false,
            terminated: false,
            terminated_at: chrono::Utc::now(),
            client_certificate: None,
            security_policy_uri: String::new(),
            authentication_token: NodeId::null(),
            secure_channel: Arc::new(RwLock::new(SecureChannel::new(server.certificate_store(), Role::Server, decoding_limits))),
            session_nonce: ByteString::null(),
            session_timeout: 0f64,
            user_identity: None,
            max_request_message_size: 0,
            max_response_message_size: 0,
            endpoint_url: UAString::null(),
            max_browse_continuation_points,
            browse_continuation_points: VecDeque::with_capacity(max_browse_continuation_points),
            diagnostics,
        };
        {
            let mut diagnostics = trace_write_lock_unwrap!(session.diagnostics);
            diagnostics.on_create_session(&session);
        }
        session
    }

    pub fn terminated(&self) -> bool { self.terminated }

    pub fn terminated_at(&self) -> DateTimeUtc { self.terminated_at.clone() }

    pub fn set_terminated(&mut self) {
        info!("Session being set to terminated");
        self.terminated = true;
        self.terminated_at = chrono::Utc::now();
    }

    pub fn enqueue_publish_request(&mut self, address_space: &AddressSpace, request_id: u32, request: PublishRequest) -> Result<(), StatusCode> {
        self.subscriptions.enqueue_publish_request(address_space, request_id, request)
    }

    pub fn tick_subscriptions(&mut self, now: &DateTimeUtc, address_space: &AddressSpace, reason: TickReason) -> Result<(), StatusCode> {
        self.subscriptions.tick(now, address_space, reason)
    }

    /// Reset the lifetime counter on the subscription, e.g. because a service references the
    /// subscription.
    pub fn reset_subscription_lifetime_counter(&mut self, subscription_id: u32) {
        if let Some(subscription) = self.subscriptions.get_mut(subscription_id) {
            subscription.reset_lifetime_counter();
        }
    }

    /// Iterates through the existing queued publish requests and creates a timeout
    /// publish response any that have expired.
    pub fn expire_stale_publish_requests(&mut self, now: &DateTimeUtc) {
        self.subscriptions.expire_stale_publish_requests(now);
    }

    pub fn add_browse_continuation_point(&mut self, continuation_point: BrowseContinuationPoint) {
        // Remove excess browse continuation points
        while self.browse_continuation_points.len() >= self.max_browse_continuation_points {
            let _ = self.browse_continuation_points.pop_front();
        }
        self.browse_continuation_points.push_back(continuation_point);
    }

    /// Find a continuation point by id. If the continuation point is out of date is removed and None
    /// is returned.
    pub fn find_browse_continuation_point(&self, id: &ByteString) -> Option<BrowseContinuationPoint> {
        let continuation_point = self.browse_continuation_points.iter().find(|continuation_point| {
            continuation_point.id.eq(id)
        });
        if let Some(continuation_point) = continuation_point {
            Some(continuation_point.clone())
        } else {
            None
        }
    }

    pub fn remove_expired_browse_continuation_points(&mut self, address_space: &AddressSpace) {
        self.browse_continuation_points.retain(|continuation_point| {
            continuation_point.is_valid_browse_continuation_point(address_space)
        });
    }

    pub fn remove_browse_continuation_point(&mut self, continuation_point_id: &ByteString) {
        self.browse_continuation_points.retain(|continuation_point| {
            !continuation_point.id.eq(continuation_point_id)
        });
    }

    /// Remove all the specified continuation points by id
    pub fn remove_browse_continuation_points(&mut self, continuation_points: &[ByteString]) {
        use std::collections::HashSet;
        // Turn the supplied slice into a set
        let continuation_points_set: HashSet<ByteString> = continuation_points.iter().cloned().collect();
        // Now remove any continuation points that are part of that set
        self.browse_continuation_points.retain(|continuation_point| {
            !continuation_points_set.contains(&continuation_point.id)
        });
    }
}

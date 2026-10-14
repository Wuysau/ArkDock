//! The single structured error contract for every Tauri command.
//!
//! Internal modules keep using `Result<T, String>`; this module is the explicit
//! conversion boundary. A wire code is either recognised by [`ERROR_TABLE`] or the
//! error degrades to `INTERNAL`, which carries a fixed message and no details so an
//! unclassified failure cannot leak paths, tokens, or foreign window titles.
//!
//! Invariants enforced by the tests below:
//! - the vocabulary here and `protocol/model.json` cannot drift apart;
//! - every recognised code keeps its own category, retryable flag and recovery hint;
//! - non-idempotent operations (Agent send/pairing, trust confirmation, exports that
//!   may already have run) are never marked retryable;
//! - a code means the same thing on every command path.

use super::model::{ErrorCategory, ErrorCode, IpcError, RecoveryAction};

/// Bounded diagnostics are truncated at a character boundary.
pub(crate) const MAX_DETAILS_CHARS: usize = 400;
/// Codes are short, uppercase, underscore separated tokens.
const MAX_CODE_CHARS: usize = 64;

#[derive(Debug)]
pub(crate) struct ErrorSpec {
    pub(crate) wire: &'static str,
    pub(crate) code: ErrorCode,
    pub(crate) category: ErrorCategory,
    pub(crate) retryable: bool,
    pub(crate) recovery: RecoveryAction,
    pub(crate) message: &'static str,
}

macro_rules! error_table {
    ($(($wire:literal, $code:ident, $category:ident, $retryable:literal, $recovery:ident, $message:literal)),* $(,)?) => {
        pub(crate) const ERROR_TABLE: &[ErrorSpec] = &[
            $(ErrorSpec {
                wire: $wire,
                code: ErrorCode::$code,
                category: ErrorCategory::$category,
                retryable: $retryable,
                recovery: RecoveryAction::$recovery,
                message: $message,
            }),*
        ];
    };
}

// Categories: validation, state, trust, environment, storage, discovery, toolchain,
// transport, selection, agent, internal.
error_table![
    // Request-level validation and authorization of the caller itself.
    (
        "UNAUTHORIZED_WINDOW",
        UnauthorizedWindow,
        Validation,
        false,
        None,
        "This window is not the ArkDock main window"
    ),
    (
        "UNAUTHORIZED_ORIGIN",
        UnauthorizedOrigin,
        Validation,
        false,
        None,
        "The caller origin is not the local ArkDock page"
    ),
    (
        "INVALID_DISPLAY_REQUEST_ID",
        InvalidDisplayRequestId,
        Validation,
        false,
        CorrectRequest,
        "The display request identifier is outside the accepted range"
    ),
    (
        "DISPLAY_REQUEST_CONFLICT",
        DisplayRequestConflict,
        Validation,
        false,
        CorrectRequest,
        "A newer display request already owns this identifier"
    ),
    (
        "INVALID_CANDIDATE",
        InvalidCandidate,
        Validation,
        false,
        CorrectRequest,
        "The project candidate is unknown or malformed"
    ),
    (
        "INVALID_PROJECT",
        InvalidProject,
        Validation,
        false,
        CorrectRequest,
        "The project identifier is unknown or malformed"
    ),
    (
        "INVALID_DIRECTORY_PATH",
        InvalidDirectoryPath,
        Validation,
        false,
        CorrectRequest,
        "Enter a bounded absolute directory path"
    ),
    (
        "ABSOLUTE_DIRECTORY_REQUIRED",
        AbsoluteDirectoryRequired,
        Validation,
        false,
        CorrectRequest,
        "Relative paths are not accepted"
    ),
    (
        "DIRECTORY_REQUIRED",
        DirectoryRequired,
        Validation,
        false,
        CorrectRequest,
        "Select or enter an existing directory"
    ),
    (
        "INVALID_NATIVE_INPUT",
        InvalidNativeInput,
        Validation,
        false,
        CorrectRequest,
        "The native input coordinates or build identity are out of range"
    ),
    (
        "INVALID_FRAME_CONSUMER",
        InvalidFrameConsumer,
        Validation,
        false,
        CorrectRequest,
        "The frame consumer identifier is empty or too long"
    ),
    (
        "INPUT_OUTSIDE_SURFACE",
        InputOutsideSurface,
        Validation,
        false,
        CorrectRequest,
        "The input point is outside the current native surface"
    ),
    (
        "EXPERIMENTAL_CONTROL_OPT_IN_REQUIRED",
        ExperimentalControlOptInRequired,
        Validation,
        false,
        None,
        "Native controls require an explicit opt-in before they can be submitted"
    ),
    (
        "NATIVE_TEXT_LIMIT",
        NativeTextLimit,
        Validation,
        false,
        CorrectRequest,
        "Commit 1 to 128 Unicode scalars; use the verified Enter key for newlines"
    ),
    (
        "NATIVE_KEY_REQUIRED",
        NativeKeyRequired,
        Validation,
        false,
        CorrectRequest,
        "This native control action requires a key"
    ),
    (
        "NATIVE_PAGE_INVALID",
        NativePageInvalid,
        Validation,
        false,
        CorrectRequest,
        "The requested page is not in the verified compiled page list"
    ),
    (
        "NATIVE_OPERATION_ID_INVALID",
        NativeOperationIdInvalid,
        Validation,
        false,
        CorrectRequest,
        "The native control operation identifier is malformed"
    ),
    (
        "NATIVE_GESTURE_ID_INVALID",
        NativeGestureIdInvalid,
        Validation,
        false,
        CorrectRequest,
        "The native gesture identifier is malformed"
    ),
    (
        "NATIVE_CONTROL_IDENTITY_INVALID",
        NativeControlIdentityInvalid,
        Validation,
        false,
        CorrectRequest,
        "The native control identity does not describe a valid build surface"
    ),
    (
        "NATIVE_CONTROL_ARGUMENT_MISMATCH",
        NativeControlArgumentMismatch,
        Validation,
        false,
        CorrectRequest,
        "The native control request carries arguments for another action"
    ),
    (
        "NATIVE_CONTROL_UNUSED_COORDINATES",
        NativeControlUnusedCoordinates,
        Validation,
        false,
        CorrectRequest,
        "This native control action does not accept coordinates"
    ),
    (
        "NATIVE_CONTROL_UNUSED_GESTURE",
        NativeControlUnusedGesture,
        Validation,
        false,
        CorrectRequest,
        "This native control action does not accept a gesture identifier"
    ),
    (
        "NATIVE_CONTROL_RESOLUTION_ARGUMENT_MISMATCH",
        NativeControlResolutionArgumentMismatch,
        Validation,
        false,
        CorrectRequest,
        "The resolution request carries arguments for another action"
    ),
    (
        "NATIVE_CONTROL_RESOLUTION_LIMIT",
        NativeControlResolutionLimit,
        Validation,
        false,
        CorrectRequest,
        "The requested resolution is outside the verified device profile range"
    ),
    (
        "LEGACY_INPUT_SURFACE_IDENTITY_REQUIRED",
        LegacyInputSurfaceIdentityRequired,
        Validation,
        false,
        None,
        "Dynamic surfaces require the typed native control channel"
    ),
    (
        "DEVICE_KIND_REQUIRED",
        DeviceKindRequired,
        Validation,
        false,
        CorrectRequest,
        "Select physical device or emulator explicitly; the transport cannot prove the kind"
    ),
    (
        "DEVICE_TARGET_REQUIRED",
        DeviceTargetRequired,
        Validation,
        false,
        CorrectRequest,
        "Select a discovered device target first"
    ),
    (
        "DEVICE_CONNECTION_CONSENT_REQUIRED",
        DeviceConnectionConsentRequired,
        Validation,
        false,
        None,
        "Explicitly allow device discovery before ArkDock contacts a device service"
    ),
    (
        "DEVICE_DEPLOYMENT_CONSENT_REQUIRED",
        DeviceDeploymentConsentRequired,
        Validation,
        false,
        None,
        "Explicitly authorize installing a signed artifact on the selected device"
    ),
    (
        "DEVICE_CAPTURE_REQUIRES_PNG_DESTINATION",
        DeviceCaptureRequiresPngDestination,
        Validation,
        false,
        CorrectRequest,
        "Device capture requires an explicit PNG destination"
    ),
    (
        "DEVICE_TEXT_ENCODING_UNVERIFIED",
        DeviceTextEncodingUnverified,
        Validation,
        false,
        CorrectRequest,
        "Only bounded ASCII token text has a reviewed device command plan"
    ),
    (
        "INVALID_DEVICE_RUNTIME_IDENTITY",
        InvalidDeviceRuntimeIdentity,
        Validation,
        false,
        CorrectRequest,
        "The device runtime identity does not match the current preview surface"
    ),
    (
        "INVALID_DEVICE_RUNTIME_SURFACE",
        InvalidDeviceRuntimeSurface,
        Validation,
        false,
        CorrectRequest,
        "The device runtime request does not describe a valid surface"
    ),
    (
        "INVALID_DEVICE_APPLICATION_IDENTIFIER",
        InvalidDeviceApplicationIdentifier,
        Validation,
        false,
        CorrectRequest,
        "The device application identifier is malformed"
    ),
    (
        "INVALID_DEVICE_CAPTURE_NONCE",
        InvalidDeviceCaptureNonce,
        Validation,
        false,
        CorrectRequest,
        "The device capture nonce is missing or malformed"
    ),
    (
        "INVALID_DEVICE_GESTURE_VELOCITY",
        InvalidDeviceGestureVelocity,
        Validation,
        false,
        CorrectRequest,
        "The device gesture velocity is outside the accepted range"
    ),
    (
        "INVALID_DEVICE_INPUT_COORDINATE",
        InvalidDeviceInputCoordinate,
        Validation,
        false,
        CorrectRequest,
        "Device input requires current display evidence and nonzero in-range coordinates"
    ),
    (
        "SELECTION_ID_INVALID",
        SelectionIdInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection identifier is malformed"
    ),
    (
        "SELECTION_INSTRUCTION_INVALID",
        SelectionInstructionInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection instruction is empty or too long"
    ),
    (
        "SELECTION_BOUNDS_INVALID",
        SelectionBoundsInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection bounds are outside the captured frame"
    ),
    (
        "SELECTION_FRAME_TIMESTAMP_INVALID",
        SelectionFrameTimestampInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection frame timestamp is not acceptable"
    ),
    (
        "SELECTION_TIMESTAMP_INVALID",
        SelectionTimestampInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection lifetime is not acceptable"
    ),
    (
        "SELECTION_DELIVERY_ID_INVALID",
        SelectionDeliveryIdInvalid,
        Validation,
        false,
        CorrectRequest,
        "The selection delivery identifier is malformed"
    ),
    (
        "SELECTION_READ_LIMIT",
        SelectionReadLimit,
        Validation,
        false,
        CorrectRequest,
        "The selection artifact exceeds the bounded read limit"
    ),
    (
        "SELECTION_CROP_LIMIT",
        SelectionCropLimit,
        Validation,
        false,
        CorrectRequest,
        "The selection crop exceeds the bounded artifact limit"
    ),
    (
        "PAGE_PATH_INVALID",
        PagePathInvalid,
        Validation,
        false,
        CorrectRequest,
        "The page path is not an accepted bounded project-relative value"
    ),
    (
        "PAGE_PROFILE_INVALID",
        PageProfileInvalid,
        Validation,
        false,
        CorrectRequest,
        "The page profile is not part of the verified compiled profile"
    ),
    (
        "PAGE_NOT_IN_COMPILED_ARTIFACT",
        PageNotInCompiledArtifact,
        Validation,
        false,
        CorrectRequest,
        "The page is not part of the verified compiled artifact"
    ),
    (
        "AGENT_INSTRUCTION_REQUIRED",
        AgentInstructionRequired,
        Validation,
        false,
        CorrectRequest,
        "Write the instruction that should be delivered to the Agent"
    ),
    (
        "AGENT_SESSION_ID_INVALID",
        AgentSessionIdInvalid,
        Validation,
        false,
        CorrectRequest,
        "The Agent session identifier is malformed"
    ),
    (
        "AGENT_TIMESTAMP_INVALID",
        AgentTimestampInvalid,
        Validation,
        false,
        CorrectRequest,
        "The Agent-provided timestamp is outside the accepted clock skew"
    ),
    (
        "AGENT_CONTEXT_INVALID",
        AgentContextInvalid,
        Validation,
        false,
        CorrectRequest,
        "The Agent request context is malformed"
    ),
    (
        "AGENT_BINDING_REQUIRED",
        AgentBindingRequired,
        Validation,
        false,
        CorrectRequest,
        "Choose an available Agent session before binding"
    ),
    (
        "AGENT_PAIR_PATH_INVALID",
        AgentPairPathInvalid,
        Validation,
        false,
        CorrectRequest,
        "Choose a local path for the pairing credential"
    ),
    (
        "PAIRING_PATH_INVALID",
        PairingPathInvalid,
        Validation,
        false,
        CorrectRequest,
        "The pairing credential path is not an accepted local path"
    ),
    (
        "ASSISTED_PROMPT_REQUIRED",
        AssistedPromptRequired,
        Validation,
        false,
        CorrectRequest,
        "The assisted fill requires the prepared prompt text"
    ),
    (
        "ASSISTED_ACTION_INVALID",
        AssistedActionInvalid,
        Validation,
        false,
        CorrectRequest,
        "The assisted action is not implemented for this command"
    ),
    (
        "EXPORT_CANCELLED",
        ExportCancelled,
        Validation,
        false,
        None,
        "The export destination was not chosen"
    ),
    (
        "DIRECTORY_SELECTION_CANCELLED",
        DirectorySelectionCancelled,
        Validation,
        false,
        None,
        "No directory was selected"
    ),
    (
        "SDK_SELECTION_CANCELLED",
        SdkSelectionCancelled,
        Validation,
        false,
        None,
        "No SDK directory was selected"
    ),
    (
        "PAIRING_SAVE_CANCELLED",
        PairingSaveCancelled,
        Validation,
        false,
        None,
        "The pairing credential was not saved and nothing was paired"
    ),
    // Session, target and transport identity.
    (
        "STALE_REQUEST",
        StaleRequest,
        State,
        false,
        RefreshSnapshot,
        "The target window or preview session changed; review the current state"
    ),
    (
        "STALE_TARGET",
        StaleTarget,
        State,
        false,
        ReselectTarget,
        "The bound target window is no longer current; dock again"
    ),
    (
        "TARGET_REQUIRED",
        TargetRequired,
        State,
        false,
        ReselectTarget,
        "Connect a target window first"
    ),
    (
        "STALE_PROJECT",
        StaleProject,
        State,
        false,
        ReselectProject,
        "The associated project is no longer current"
    ),
    (
        "PROJECT_REQUIRED",
        ProjectRequired,
        State,
        false,
        ReselectProject,
        "Associate a project for this target window first"
    ),
    (
        "STALE_SESSION",
        StaleSession,
        State,
        false,
        RestartPreview,
        "This preview session was replaced; restart the preview"
    ),
    (
        "PREVIEW_SESSION_REQUIRED",
        PreviewSessionRequired,
        State,
        false,
        RestartPreview,
        "An active preview session is required"
    ),
    (
        "STALE_INPUT",
        StaleInput,
        State,
        false,
        RestartPreview,
        "Input requires the current native surface of the latest build"
    ),
    (
        "STALE_DISPLAY_REQUEST",
        StaleDisplayRequest,
        State,
        true,
        RefreshSnapshot,
        "A newer display request already exists"
    ),
    (
        "FRAME_UNAVAILABLE",
        FrameUnavailable,
        State,
        true,
        RetryRequest,
        "No native frame is available yet"
    ),
    (
        "STALE_FRAME",
        StaleFrame,
        State,
        false,
        RestartPreview,
        "The retained frame does not belong to the current build"
    ),
    (
        "STALE_FRAME_CONSUMER",
        StaleFrameConsumer,
        State,
        false,
        RestartPreview,
        "This frame consumer was replaced by a newer one"
    ),
    (
        "BINARY_TRANSPORT_NOT_ENABLED",
        BinaryTransportNotEnabled,
        State,
        false,
        RestartPreview,
        "Binary frame transport is not enabled for this session"
    ),
    (
        "BUSY",
        Busy,
        State,
        true,
        AwaitCurrentOperation,
        "The backend is busy; retry after the current operation settles"
    ),
    (
        "REQUEST_QUEUE_BUSY",
        RequestQueueBusy,
        State,
        true,
        AwaitCurrentOperation,
        "The request queue is full; retry shortly"
    ),
    (
        "REQUEST_TIMEOUT",
        RequestTimeout,
        State,
        false,
        RefreshSnapshot,
        "The backend did not answer in time; the request may have started, so verify the state"
    ),
    (
        "REQUEST_WORKER_FAILED",
        RequestWorkerFailed,
        Internal,
        false,
        RefreshSnapshot,
        "The backend request worker stopped; reload the state"
    ),
    (
        "DATA_REQUEST_TIMEOUT",
        DataRequestTimeout,
        State,
        false,
        RefreshSnapshot,
        "The operation did not report a result in time; verify the current state"
    ),
    (
        "DATA_REPLY_UNAVAILABLE",
        DataReplyUnavailable,
        Internal,
        true,
        RefreshSnapshot,
        "The operation result could not be delivered"
    ),
    (
        "DATA_REPLY_INVALID",
        DataReplyInvalid,
        Internal,
        false,
        ReadDiagnostics,
        "The operation result could not be decoded"
    ),
    (
        "DATA_REPLY_TYPE_MISMATCH",
        DataReplyTypeMismatch,
        Internal,
        false,
        ReadDiagnostics,
        "The operation returned a different result kind than expected"
    ),
    (
        "DATA_SERIALIZE_FAILED",
        DataSerializeFailed,
        Internal,
        false,
        ReadDiagnostics,
        "The operation result could not be serialized"
    ),
    (
        "INPUT_QUEUE_BUSY",
        InputQueueBusy,
        State,
        true,
        AwaitCurrentOperation,
        "Native input is already queued; retry after it settles"
    ),
    (
        "INSPECTOR_QUERY_PENDING",
        InspectorQueryPending,
        State,
        true,
        AwaitCurrentOperation,
        "An Inspector query is already running"
    ),
    (
        "INSPECTOR_QUERY_THROTTLED",
        InspectorQueryThrottled,
        State,
        true,
        AwaitCurrentOperation,
        "Inspector queries are rate limited"
    ),
    (
        "INSPECTOR_QUERY_TIMEOUT",
        InspectorQueryTimeout,
        Transport,
        true,
        RetryRequest,
        "The Inspector runtime did not answer the query in time"
    ),
    (
        "INSPECTOR_QUERY_CANCELLED",
        InspectorQueryCancelled,
        State,
        true,
        RetryRequest,
        "The Inspector query was cancelled before it completed"
    ),
    (
        "STALE_INSPECTOR_SESSION",
        StaleInspectorSession,
        State,
        false,
        RefreshSnapshot,
        "The Inspector session changed; query the tree again"
    ),
    (
        "STALE_INSPECTOR_SELECTION",
        StaleInspectorSelection,
        State,
        false,
        RetryRequest,
        "The selected component is not in the current Inspector tree"
    ),
    (
        "STALE_SELECTION_FRAME",
        StaleSelectionFrame,
        State,
        false,
        RetryRequest,
        "The selection frame is not the current displayed build"
    ),
    (
        "DISPLAYED_FRAME_EXPIRED",
        DisplayedFrameExpired,
        State,
        false,
        RetryRequest,
        "The displayed frame was replaced; select again"
    ),
    (
        "INSPECTOR_IMAGE_CHANGED",
        InspectorImageChanged,
        State,
        true,
        RetryRequest,
        "The picture changed; refresh the component tree"
    ),
    (
        "NATIVE_CURRENT_ROUTE_UNVERIFIED",
        NativeCurrentRouteUnverified,
        State,
        true,
        RefreshSnapshot,
        "The current route cannot be confirmed yet"
    ),
    (
        "NATIVE_CONTROL_STALE_IDENTITY",
        NativeControlStaleIdentity,
        State,
        false,
        RestartPreview,
        "The native control identity belongs to a replaced build"
    ),
    (
        "NATIVE_CONTROL_CHANGED_BEFORE_ENQUEUE",
        NativeControlChangedBeforeEnqueue,
        State,
        true,
        RetryRequest,
        "The target changed before the native control could be queued"
    ),
    (
        "NATIVE_CONTROL_QUEUE_BUSY",
        NativeControlQueueBusy,
        State,
        true,
        AwaitCurrentOperation,
        "The native control queue is busy; cancellation uses its own slot"
    ),
    (
        "NATIVE_CONTROL_CANCELLING",
        NativeControlCancelling,
        State,
        true,
        AwaitCurrentOperation,
        "A native control cancellation is already settling"
    ),
    (
        "NATIVE_CONTROL_PAUSED",
        NativeControlPaused,
        State,
        false,
        RestartPreview,
        "Native control is paused for the current surface"
    ),
    (
        "NATIVE_CONTROL_WINDOW_HIDDEN",
        NativeControlWindowHidden,
        State,
        false,
        None,
        "The preview window is hidden; show it before sending input"
    ),
    (
        "NATIVE_CONTROL_SURFACE_PENDING",
        NativeControlSurfacePending,
        State,
        true,
        AwaitCurrentOperation,
        "The native surface is not interactive yet"
    ),
    (
        "STALE_CONTROL_IDENTITY",
        StaleControlIdentity,
        State,
        false,
        RestartPreview,
        "The control identity belongs to a replaced preview session"
    ),
    (
        "STALE_CONTROL_SURFACE",
        StaleControlSurface,
        State,
        false,
        RestartPreview,
        "The native control surface was replaced"
    ),
    (
        "STALE_DEVICE_RUNTIME_IDENTITY",
        StaleDeviceRuntimeIdentity,
        State,
        false,
        RestartPreview,
        "The device runtime identity belongs to a replaced preview session"
    ),
    (
        "STALE_DEVICE_TARGET_SELECTION",
        StaleDeviceTargetSelection,
        State,
        false,
        ReselectProject,
        "The selected device target is no longer observed"
    ),
    (
        "DEVICE_RUNTIME_SESSION_REQUIRED",
        DeviceRuntimeSessionRequired,
        State,
        false,
        ReselectProject,
        "Select a trusted active project before using the device runtime"
    ),
    (
        "DEVICE_TARGET_SELECTION_REQUIRED",
        DeviceTargetSelectionRequired,
        State,
        false,
        ReselectProject,
        "Select a discovered device target first"
    ),
    (
        "DEVICE_TARGET_NOT_OBSERVED",
        DeviceTargetNotObserved,
        State,
        false,
        RefreshSnapshot,
        "Discovery must observe this exact target before it can be used"
    ),
    (
        "DEVICE_PROFILE_REQUIRES_CONTROLLED_RESTART",
        DeviceProfileRequiresControlledRestart,
        State,
        false,
        RestartPreview,
        "This device profile requires a controlled preview restart"
    ),
    (
        "NATIVE_SURFACE_CLOSED",
        NativeSurfaceClosed,
        State,
        false,
        RestartPreview,
        "The native surface closed"
    ),
    (
        "CURRENT_FRAME_REQUIRED",
        CurrentFrameRequired,
        State,
        false,
        RetryRequest,
        "A current native frame is required"
    ),
    (
        "CURRENT_NATIVE_FRAME_REQUIRED",
        CurrentNativeFrameRequired,
        State,
        false,
        RetryRequest,
        "A current native frame is required"
    ),
    (
        "LAST_SUCCESS_REQUIRED",
        LastSuccessRequired,
        State,
        false,
        RestartPreview,
        "No successful build exists to retain as the stale picture"
    ),
    // Execution trust.
    (
        "TRUST_REQUIRED",
        TrustRequired,
        Trust,
        false,
        ConfirmTrust,
        "Project execution has not been authorized"
    ),
    (
        "TRUST_FINGERPRINT_CHANGED",
        TrustFingerprintChanged,
        Trust,
        false,
        ConfirmTrust,
        "The build configuration changed; confirm trust again"
    ),
    (
        "AGENT_LOCAL_TRUSTED_WORKSPACE_REQUIRED",
        AgentLocalTrustedWorkspaceRequired,
        Trust,
        false,
        ConfirmTrust,
        "The Agent bridge needs a local trusted workspace"
    ),
    // Host environment and windowing.
    (
        "UNSUPPORTED_ENVIRONMENT",
        UnsupportedEnvironment,
        Environment,
        false,
        ReselectProject,
        "Native preview supports Windows-local projects only"
    ),
    (
        "UNSUPPORTED_PLATFORM",
        UnsupportedPlatform,
        Environment,
        false,
        None,
        "This action requires a Windows interactive desktop"
    ),
    (
        "WINDOWING_UNAVAILABLE",
        WindowingUnavailable,
        Environment,
        false,
        ReadDiagnostics,
        "The native window controller is unavailable"
    ),
    (
        "TARGET_PLACEMENT_UNAVAILABLE",
        TargetPlacementUnavailable,
        Environment,
        false,
        ReadDiagnostics,
        "The target window cannot be placed right now"
    ),
    (
        "WORK_AREA_TOO_SMALL",
        WorkAreaTooSmall,
        Environment,
        false,
        None,
        "Two windows cannot fit their minimum dimensions side by side"
    ),
    (
        "WORK_AREA_UNAVAILABLE",
        WorkAreaUnavailable,
        Environment,
        false,
        ReadDiagnostics,
        "The monitor work area could not be read"
    ),
    (
        "DISPLAY_SUSPENDED",
        DisplaySuspended,
        Environment,
        false,
        None,
        "Expand ArkDock before arranging windows"
    ),
    (
        "LAYOUT_CANCELLATION_PENDING",
        LayoutCancellationPending,
        Environment,
        true,
        AwaitCurrentOperation,
        "Wait for accepted layout consent requests to settle"
    ),
    (
        "FINISH_DRAG_FIRST",
        FinishDragFirst,
        Environment,
        false,
        None,
        "Finish the current drag before starting another one"
    ),
    (
        "DEVECO_GUI_RUNNING",
        DevecoGuiRunning,
        Environment,
        false,
        None,
        "DevEco Studio is running; close it before independent acceptance"
    ),
    (
        "DEVICE_RUNTIME_UNSUPPORTED_PLATFORM",
        DeviceRuntimeUnsupportedPlatform,
        Environment,
        false,
        None,
        "The device runtime adapter is Windows-local only"
    ),
    (
        "SELECTION_NATIVE_PLATFORM_UNSUPPORTED",
        SelectionNativePlatformUnsupported,
        Environment,
        false,
        None,
        "Native selection requires a supported desktop platform"
    ),
    // Host storage and settings.
    (
        "PERSISTENCE_UNAVAILABLE",
        PersistenceUnavailable,
        Storage,
        false,
        ReadDiagnostics,
        "Project settings storage is unavailable"
    ),
    (
        "SETTINGS_DIRECTORY",
        SettingsDirectory,
        Storage,
        false,
        ReadDiagnostics,
        "The settings directory could not be prepared"
    ),
    (
        "SETTINGS_INVALID",
        SettingsInvalid,
        Storage,
        false,
        ReadDiagnostics,
        "The settings file is invalid; the original file was preserved"
    ),
    (
        "SETTINGS_VERSION_UNSUPPORTED",
        SettingsVersionUnsupported,
        Storage,
        false,
        ReadDiagnostics,
        "The settings file version is not supported"
    ),
    (
        "SETTINGS_LIMIT_EXCEEDED",
        SettingsLimitExceeded,
        Storage,
        false,
        ReadDiagnostics,
        "The settings file exceeds its bounded size"
    ),
    (
        "SETTINGS_SYMLINK_REJECTED",
        SettingsSymlinkRejected,
        Storage,
        false,
        ReadDiagnostics,
        "A symbolic link is not accepted as the settings file"
    ),
    (
        "SETTINGS_REPLACE",
        SettingsReplace,
        Storage,
        true,
        RetryRequest,
        "The settings file could not be replaced atomically"
    ),
    (
        "SETTINGS_UNAVAILABLE",
        SettingsUnavailable,
        Storage,
        false,
        ReadDiagnostics,
        "Settings could not be loaded at startup"
    ),
    (
        "TRUST_RECORD_LIMIT",
        TrustRecordLimit,
        Storage,
        false,
        ReadDiagnostics,
        "The trust record limit was reached; revoke unused records first"
    ),
    (
        "SDK_PATH_INACCESSIBLE",
        SdkPathInaccessible,
        Storage,
        false,
        ConfigureSdk,
        "The selected SDK path is not accessible"
    ),
    (
        "SDK_PATH_NOT_DIRECTORY",
        SdkPathNotDirectory,
        Storage,
        false,
        ConfigureSdk,
        "The selected SDK path is not a directory"
    ),
    (
        "SELECTION_STORAGE_UNAVAILABLE",
        SelectionStorageUnavailable,
        Storage,
        false,
        ReadDiagnostics,
        "Selection storage is unavailable"
    ),
    (
        "SELECTION_APP_DATA_UNAVAILABLE",
        SelectionAppDataUnavailable,
        Storage,
        false,
        ReadDiagnostics,
        "The application data directory is unavailable for selections"
    ),
    (
        "SELECTION_DIRECTORY_UNAVAILABLE",
        SelectionDirectoryUnavailable,
        Storage,
        false,
        ReadDiagnostics,
        "The selection directory is unavailable"
    ),
    (
        "SELECTION_DIRECTORY_UNSAFE",
        SelectionDirectoryUnsafe,
        Storage,
        false,
        ReadDiagnostics,
        "The selection directory failed its safety checks"
    ),
    (
        "SELECTION_PATH_UNSAFE",
        SelectionPathUnsafe,
        Storage,
        false,
        ReadDiagnostics,
        "The selection artifact path failed its safety checks"
    ),
    (
        "SELECTION_READ_FAILED",
        SelectionReadFailed,
        Storage,
        true,
        RetryRequest,
        "The selection artifact could not be read"
    ),
    (
        "SELECTION_WRITE_FAILED",
        SelectionWriteFailed,
        Storage,
        true,
        RetryRequest,
        "The selection artifact could not be written"
    ),
    (
        "SELECTION_ARTIFACT_INVALID",
        SelectionArtifactInvalid,
        Storage,
        false,
        ReadDiagnostics,
        "The stored selection artifact is invalid"
    ),
    (
        "SELECTION_CLEANUP_FAILED",
        SelectionCleanupFailed,
        Storage,
        true,
        RetryRequest,
        "Expired selections could not be cleaned up"
    ),
    (
        "EXPORT_WRITE_FAILED",
        ExportWriteFailed,
        Storage,
        true,
        RetryRequest,
        "The export file could not be written"
    ),
    (
        "EXPORT_WORKER_FAILED",
        ExportWorkerFailed,
        Internal,
        true,
        RetryRequest,
        "The export worker stopped unexpectedly"
    ),
    (
        "PAIRING_LOCAL_DISK_REQUIRED",
        PairingLocalDiskRequired,
        Storage,
        false,
        None,
        "The pairing credential must be written to a local disk path"
    ),
    (
        "PAIRING_PRIVATE_ACL_FAILED",
        PairingPrivateAclFailed,
        Storage,
        false,
        ReadDiagnostics,
        "The pairing credential could not be restricted to the current user"
    ),
    (
        "PAIRING_RANDOM_FAILED",
        PairingRandomFailed,
        Storage,
        false,
        RetryRequest,
        "No secure random source was available for the pairing credential"
    ),
    (
        "PAIRING_REPLACE_FAILED",
        PairingReplaceFailed,
        Storage,
        true,
        RetryRequest,
        "The pairing credential could not be replaced"
    ),
    (
        "PAIRING_ROLLBACK_FAILED",
        PairingRollbackFailed,
        Storage,
        false,
        ReadDiagnostics,
        "The pairing credential rollback failed; inspect the credential directory"
    ),
    (
        "PAIRING_SERIALIZE_FAILED",
        PairingSerializeFailed,
        Internal,
        false,
        ReadDiagnostics,
        "The pairing credential could not be serialized"
    ),
    (
        "PAIRING_WRITE_FAILED",
        PairingWriteFailed,
        Storage,
        false,
        ReadDiagnostics,
        "The pairing credential write did not settle; nothing was paired"
    ),
    (
        "PAIRING_WRITE_TIMEOUT",
        PairingWriteTimeout,
        Storage,
        false,
        ReadDiagnostics,
        "The pairing credential write timed out; nothing was paired"
    ),
    (
        "PAIRING_CANCELLED",
        PairingCancelled,
        State,
        false,
        None,
        "The pending pairing was cancelled; nothing was paired"
    ),
    // Read-only project discovery.
    (
        "CANDIDATE_LIMIT",
        CandidateLimit,
        Discovery,
        false,
        Rediscover,
        "The candidate list is full; rediscover before inspecting more directories"
    ),
    (
        "CANDIDATE_DISAPPEARED",
        CandidateDisappeared,
        Discovery,
        false,
        Rediscover,
        "The candidate disappeared before it could be associated"
    ),
    (
        "PROJECT_CHANGED_OR_INVALID",
        ProjectChangedOrInvalid,
        Discovery,
        false,
        Rediscover,
        "The project changed or is no longer valid; rediscover it"
    ),
    (
        "WORKSPACE_CONFLICT",
        WorkspaceConflict,
        Discovery,
        false,
        Rediscover,
        "The host's current project differs from this directory; rediscover its current workspace"
    ),
    (
        "PROJECT_INACCESSIBLE",
        ProjectInaccessible,
        Discovery,
        false,
        ReselectProject,
        "The project root is not an accessible directory"
    ),
    (
        "BUILD_PROFILE_INVALID",
        BuildProfileInvalid,
        Discovery,
        false,
        ReselectProject,
        "The build profile could not be read as JSON5"
    ),
    (
        "MODULE_LIMIT_EXCEEDED",
        ModuleLimitExceeded,
        Discovery,
        false,
        ReselectProject,
        "The build profile declares more modules than the bounded limit"
    ),
    (
        "MODULE_PATH_MISSING",
        ModulePathMissing,
        Discovery,
        false,
        ReselectProject,
        "A module in the build profile has no source path"
    ),
    (
        "MODULE_OUTSIDE_PROJECT",
        ModuleOutsideProject,
        Discovery,
        false,
        ReselectProject,
        "A module source path escapes the project root"
    ),
    (
        "CONFIG_OUTSIDE_PROJECT",
        ConfigOutsideProject,
        Discovery,
        false,
        ReselectProject,
        "A configuration file resolves outside the project root"
    ),
    (
        "CONFIG_TOO_LARGE",
        ConfigTooLarge,
        Discovery,
        false,
        ReselectProject,
        "A configuration file exceeds its bounded read limit"
    ),
    (
        "CONFIG_SYMLINK_REJECTED",
        ConfigSymlinkRejected,
        Discovery,
        false,
        ReselectProject,
        "A configuration file is a symbolic link"
    ),
    // Toolchain and native runtime availability.
    (
        "SDK_MISSING",
        SdkMissing,
        Toolchain,
        false,
        ConfigureSdk,
        "No usable HarmonyOS SDK is configured"
    ),
    (
        "SDK_CHANGED",
        SdkChanged,
        Toolchain,
        false,
        ConfigureSdk,
        "The verified SDK changed; select it again"
    ),
    (
        "PROTOCOL_UNVERIFIED",
        ProtocolUnverified,
        Toolchain,
        false,
        ReadDiagnostics,
        "This SDK version's build, pipe, frame and input protocol is not verified"
    ),
    (
        "PREVIEW_BLOCKED",
        PreviewBlocked,
        Toolchain,
        false,
        ConfigureSdk,
        "Native preview is blocked for the current toolchain"
    ),
    (
        "PREVIEW_FAILED",
        PreviewFailed,
        Toolchain,
        false,
        ReadDiagnostics,
        "The native preview build failed"
    ),
    (
        "PREVIEW_CLEANUP",
        PreviewCleanup,
        Toolchain,
        true,
        RestartPreview,
        "Preview resources could not be cleaned up completely"
    ),
    (
        "NATIVE_RUNTIME_FAILED",
        NativeRuntimeFailed,
        Toolchain,
        false,
        RestartPreview,
        "The owned native preview runtime failed"
    ),
    (
        "INSPECTOR_UNAVAILABLE",
        InspectorUnavailable,
        Toolchain,
        false,
        ReadDiagnostics,
        "The Inspector channel is unavailable"
    ),
    (
        "PROFILE_UNKNOWN",
        ProfileUnknown,
        Toolchain,
        false,
        ReadDiagnostics,
        "The device profile could not be identified"
    ),
    (
        "PROFILE_NOT_ADMITTED",
        ProfileNotAdmitted,
        Toolchain,
        false,
        None,
        "This device profile is not admitted for the current baseline"
    ),
    (
        "PROFILE_RESTART_BLOCKED",
        ProfileRestartBlocked,
        Toolchain,
        false,
        None,
        "Non-baseline profile restart and rollback are not available"
    ),
    (
        "PROFILE_RUNTIME_DEPENDENCIES_UNVERIFIED",
        ProfileRuntimeDependenciesUnverified,
        Toolchain,
        false,
        ReadDiagnostics,
        "The profile's device resources are not verified for this Stage project"
    ),
    (
        "COMPILED_PAGE_PROFILE_REQUIRED",
        CompiledPageProfileRequired,
        Toolchain,
        false,
        RestartPreview,
        "No verifiable compiled page profile was found"
    ),
    (
        "COMPILED_PAGE_PROFILE_CHANGED",
        CompiledPageProfileChanged,
        Toolchain,
        false,
        RestartPreview,
        "The compiled page profile changed; refresh the preview"
    ),
    (
        "COMPILED_PAGES_ARTIFACT_REQUIRED",
        CompiledPagesArtifactRequired,
        Toolchain,
        false,
        RestartPreview,
        "A verified compiled artifact is required before pages can change"
    ),
    (
        "STAGE_PAGE_PROFILE_REQUIRED",
        StagePageProfileRequired,
        Toolchain,
        false,
        RestartPreview,
        "The Stage page profile is required for page switching"
    ),
    (
        "SINGLE_PAGE_PROJECT",
        SinglePageProject,
        Toolchain,
        false,
        None,
        "This project has only the entry page; use preview refresh instead"
    ),
    (
        "UNSUPPORTED_BUILD_TARGET",
        UnsupportedBuildTarget,
        Toolchain,
        false,
        ReadDiagnostics,
        "The verified profile supports the default product/target and named Stage modules only"
    ),
    (
        "DEVICE_DEPLOYMENT_BLOCKED",
        DeviceDeploymentBlocked,
        Toolchain,
        false,
        ReadDiagnostics,
        "Deployment needs an installable artifact, signature evidence and a single-use grant"
    ),
    (
        "DEVICE_CAPTURE_BLOCKED",
        DeviceCaptureBlocked,
        Toolchain,
        false,
        ReadDiagnostics,
        "Device capture lacks foreground provenance and explicit screen consent"
    ),
    (
        "DEVICE_INSTALL_REQUIRES_HAP",
        DeviceInstallRequiresHap,
        Toolchain,
        false,
        ReadDiagnostics,
        "Installation requires a verified installable HAP"
    ),
    (
        "DEVICE_START_BLOCKED",
        DeviceStartBlocked,
        Toolchain,
        false,
        ReadDiagnostics,
        "No verified installed artifact establishes the foreground application"
    ),
    (
        "DEVICE_STOP_BLOCKED",
        DeviceStopBlocked,
        Toolchain,
        false,
        ReadDiagnostics,
        "ArkDock has not started a verified device application"
    ),
    (
        "FULL_EMULATOR_RUNTIME_UNAVAILABLE",
        FullEmulatorRuntimeUnavailable,
        Toolchain,
        false,
        ReadDiagnostics,
        "A compatible legally installed full emulator runtime was not verified"
    ),
    // Transport.
    (
        "PROTOCOL_VERSION_UNSUPPORTED",
        ProtocolVersionUnsupported,
        Transport,
        false,
        RestartPreview,
        "The frame protocol version is not supported"
    ),
    (
        "NATIVE_CONTROL_TOTAL_TIMEOUT",
        NativeControlTotalTimeout,
        Transport,
        true,
        RestartPreview,
        "Native control did not settle before its total timeout"
    ),
    (
        "NATIVE_CONTROL_CHANNEL_TIMEOUT",
        NativeControlChannelTimeout,
        Transport,
        false,
        RestartPreview,
        "The owned native control channel stopped answering"
    ),
    (
        "NATIVE_CONTROL_MAILBOX",
        NativeControlMailbox,
        Transport,
        false,
        RestartPreview,
        "The native control mailbox is unavailable"
    ),
    (
        "INSPECTOR_CHANNEL_UNVERIFIED_AFTER_TIMEOUT",
        InspectorChannelUnverifiedAfterTimeout,
        Transport,
        false,
        RestartPreview,
        "The Inspector channel is unverified after a timeout; restart the runtime"
    ),
    (
        "DEVICE_TARGET_NOT_CONNECTED",
        DeviceTargetNotConnected,
        Transport,
        false,
        None,
        "Reconnect the selected target on the device"
    ),
    (
        "AGENT_TRANSPORT_UNAVAILABLE",
        AgentTransportUnavailable,
        Transport,
        false,
        ReadDiagnostics,
        "No implemented Agent transport is available"
    ),
    (
        "AGENT_TRANSPORT_UNSUPPORTED",
        AgentTransportUnsupported,
        Transport,
        false,
        None,
        "This Agent transport is unsupported"
    ),
    (
        "AGENT_TRANSPORT_LEASE_EXPIRED",
        AgentTransportLeaseExpired,
        Transport,
        false,
        RefreshSnapshot,
        "The Agent transport lease expired; re-verify the session"
    ),
    (
        "AGENT_TRANSPORT_DELIVERY_LIMIT_REPAIR_REQUIRED",
        AgentTransportDeliveryLimitRepairRequired,
        Transport,
        false,
        ReadDiagnostics,
        "The Agent transport delivery limit was reached; repair the binding"
    ),
    // Selection lifecycle.
    (
        "SELECTION_NOT_FOUND",
        SelectionNotFound,
        Selection,
        false,
        RetryRequest,
        "The selection is unknown or was already cleared"
    ),
    (
        "SELECTION_EXPIRED",
        SelectionExpired,
        Selection,
        false,
        RetryRequest,
        "The selection expired; select the region again"
    ),
    (
        "SELECTION_EXPIRED_BEFORE_DISPATCH",
        SelectionExpiredBeforeDispatch,
        Selection,
        false,
        RetryRequest,
        "The selection expired before dispatch; select again"
    ),
    (
        "SELECTION_SCOPE_MISMATCH",
        SelectionScopeMismatch,
        Selection,
        false,
        RefreshSnapshot,
        "The selection belongs to another target or session"
    ),
    (
        "SELECTION_CONTEXT_CHANGED",
        SelectionContextChanged,
        Selection,
        false,
        RefreshSnapshot,
        "The selection context changed"
    ),
    (
        "SELECTION_CONTEXT_INVALID",
        SelectionContextInvalid,
        Selection,
        false,
        CorrectRequest,
        "The prepared selection context is invalid"
    ),
    (
        "SELECTION_CONTEXT_LIMIT",
        SelectionContextLimit,
        Selection,
        false,
        ReadDiagnostics,
        "The selection context limit was reached; clear old selections"
    ),
    (
        "SELECTION_SESSION_REQUIRED",
        SelectionSessionRequired,
        Selection,
        false,
        RestartPreview,
        "An active preview session is required for selections"
    ),
    (
        "SELECTION_WORKSPACE_REQUIRED",
        SelectionWorkspaceRequired,
        Selection,
        false,
        ReselectProject,
        "A trusted associated project is required for selections"
    ),
    (
        "SELECTION_FRAME_INVALID",
        SelectionFrameInvalid,
        Selection,
        false,
        RetryRequest,
        "The selection frame metadata is invalid"
    ),
    (
        "SELECTION_FRAME_MISMATCH",
        SelectionFrameMismatch,
        Selection,
        false,
        RetryRequest,
        "The selection frame does not match the displayed build"
    ),
    (
        "SELECTION_PREVIEW_NOT_CURRENT",
        SelectionPreviewNotCurrent,
        Selection,
        false,
        RetryRequest,
        "The preview moved past this selection"
    ),
    (
        "SELECTION_BUILD_STALE",
        SelectionBuildStale,
        Selection,
        false,
        RestartPreview,
        "The selected picture is stale relative to the current code"
    ),
    (
        "SELECTION_NODE_MISMATCH",
        SelectionNodeMismatch,
        Selection,
        false,
        RefreshSnapshot,
        "The selected component changed; refresh the component tree"
    ),
    (
        "SELECTION_GRANT_INVALID",
        SelectionGrantInvalid,
        Selection,
        false,
        RetryRequest,
        "The delivery grant for this selection is invalid or was revoked"
    ),
    (
        "SELECTION_IMAGE_INTEGRITY",
        SelectionImageIntegrity,
        Internal,
        false,
        ReadDiagnostics,
        "The captured selection image failed its integrity check"
    ),
    (
        "SELECTION_JPEG_INVALID",
        SelectionJpegInvalid,
        Internal,
        false,
        ReadDiagnostics,
        "The captured selection image is not a valid JPEG"
    ),
    (
        "SELECTION_JPEG_DIMENSIONS",
        SelectionJpegDimensions,
        Internal,
        false,
        ReadDiagnostics,
        "The captured selection image dimensions do not match the frame"
    ),
    (
        "SELECTION_CROP_FAILED",
        SelectionCropFailed,
        Internal,
        false,
        ReadDiagnostics,
        "The selection crop could not be produced"
    ),
    // Agent sessions, pairing and delivery. These operations are not idempotent, so
    // none of them is retryable and none suggests an automatic retry.
    (
        "AGENT_ADAPTER_NOT_IMPLEMENTED",
        AgentAdapterNotImplemented,
        Agent,
        false,
        None,
        "No adapter implements this Agent kind"
    ),
    (
        "AGENT_BINDING_EXPIRED",
        AgentBindingExpired,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent binding expired; verify the session again"
    ),
    (
        "AGENT_BINDING_LIMIT",
        AgentBindingLimit,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent binding limit was reached"
    ),
    (
        "AGENT_BINDING_NOT_FOUND",
        AgentBindingNotFound,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent binding is unknown or expired"
    ),
    (
        "AGENT_BUSY_RETRY_AFTER_CURRENT_TURN",
        AgentBusyRetryAfterCurrentTurn,
        Agent,
        false,
        AwaitCurrentOperation,
        "The Agent is busy; retry manually after the current turn"
    ),
    (
        "AGENT_DELIVERY_LIMIT_WAIT_FOR_SELECTION_EXPIRY",
        AgentDeliveryLimitWaitForSelectionExpiry,
        Agent,
        false,
        AwaitCurrentOperation,
        "The Agent delivery limit was reached; wait for a selection to expire"
    ),
    (
        "AGENT_DELIVERY_NOT_FOUND",
        AgentDeliveryNotFound,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent delivery is unknown"
    ),
    (
        "AGENT_PAIR_ALREADY_USED",
        AgentPairAlreadyUsed,
        Agent,
        false,
        None,
        "This pairing credential was already activated"
    ),
    (
        "AGENT_PAIR_EXPIRED",
        AgentPairExpired,
        Agent,
        false,
        None,
        "The pending pairing expired; nothing was paired"
    ),
    (
        "AGENT_PAIR_EXPIRED_OR_WRONG_KIND",
        AgentPairExpiredOrWrongKind,
        Agent,
        false,
        None,
        "The pairing credential expired or names another Agent kind"
    ),
    (
        "AGENT_PAIR_PREPARE_REQUIRED",
        AgentPairPrepareRequired,
        Agent,
        false,
        None,
        "Prepare the pairing before activating it"
    ),
    (
        "AGENT_PAIR_PREPARE_SUPERSEDED",
        AgentPairPrepareSuperseded,
        Agent,
        false,
        None,
        "A newer pairing preparation replaced this one"
    ),
    (
        "AGENT_PAIR_REQUIRED",
        AgentPairRequired,
        Agent,
        false,
        ReadDiagnostics,
        "No active Agent pairing credential"
    ),
    (
        "AGENT_PAIR_REVOKED",
        AgentPairRevoked,
        Agent,
        false,
        None,
        "The pairing credential was revoked"
    ),
    (
        "AGENT_PAIR_SCOPE_CHANGED",
        AgentPairScopeChanged,
        Agent,
        false,
        RefreshSnapshot,
        "The target or session changed during pairing; nothing was paired"
    ),
    (
        "AGENT_PAIRING_IN_PROGRESS",
        AgentPairingInProgress,
        Agent,
        false,
        AwaitCurrentOperation,
        "A pairing write is still settling; retry manually afterwards"
    ),
    (
        "AGENT_PREVIEW_SESSION_REQUIRED",
        AgentPreviewSessionRequired,
        Agent,
        false,
        RestartPreview,
        "An active preview session is required for Agent delivery"
    ),
    (
        "AGENT_RECEIPT_INVALID",
        AgentReceiptInvalid,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent receipt is invalid"
    ),
    (
        "AGENT_RECEIPT_MISMATCH",
        AgentReceiptMismatch,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent receipt does not match the submitted delivery"
    ),
    (
        "AGENT_RECEIPT_SCOPE_MISMATCH",
        AgentReceiptScopeMismatch,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent receipt names another scope"
    ),
    (
        "AGENT_RECEIPT_TRANSITION_INVALID",
        AgentReceiptTransitionInvalid,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent receipt transition is not allowed"
    ),
    (
        "AGENT_REGISTRATION_EXPIRED",
        AgentRegistrationExpired,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent registration expired"
    ),
    (
        "AGENT_REGISTRATION_INVALID",
        AgentRegistrationInvalid,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent registration is invalid"
    ),
    (
        "AGENT_REGISTRATION_REQUIRED",
        AgentRegistrationRequired,
        Agent,
        false,
        RefreshSnapshot,
        "No matching Agent registration was observed"
    ),
    (
        "AGENT_REGISTRATION_SCOPE_INVALID",
        AgentRegistrationScopeInvalid,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent registration names another scope"
    ),
    (
        "AGENT_REQUEST_DEADLINE",
        AgentRequestDeadline,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent request passed its deadline"
    ),
    (
        "AGENT_ROUTE_NOT_FOUND",
        AgentRouteNotFound,
        Agent,
        false,
        None,
        "No Agent route matches this request"
    ),
    (
        "AGENT_RUNTIME_LEASE_EXPIRED",
        AgentRuntimeLeaseExpired,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent runtime lease expired"
    ),
    (
        "AGENT_RUNTIME_UNAVAILABLE",
        AgentRuntimeUnavailable,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent runtime is unavailable"
    ),
    (
        "AGENT_SELECTION_CHANGED",
        AgentSelectionChanged,
        Agent,
        false,
        RefreshSnapshot,
        "The selection changed before delivery"
    ),
    (
        "AGENT_SELECTION_SCOPE_MISMATCH",
        AgentSelectionScopeMismatch,
        Agent,
        false,
        RefreshSnapshot,
        "The selection belongs to another session"
    ),
    (
        "AGENT_SESSION_CHANGED_REPAIR_REQUIRED",
        AgentSessionChangedRepairRequired,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent session changed; repair the binding"
    ),
    (
        "AGENT_SESSION_MISMATCH",
        AgentSessionMismatch,
        Agent,
        false,
        RefreshSnapshot,
        "The Agent session does not match this binding"
    ),
    (
        "AGENT_TARGET_LIFECYCLE_MISMATCH",
        AgentTargetLifecycleMismatch,
        Agent,
        false,
        ReselectTarget,
        "The target window lifecycle changed; dock again"
    ),
    (
        "AGENT_TARGET_REQUIRED",
        AgentTargetRequired,
        Agent,
        false,
        ReselectTarget,
        "Connect a target window first"
    ),
    (
        "AGENT_WORKSPACE_MISMATCH",
        AgentWorkspaceMismatch,
        Agent,
        false,
        ReselectProject,
        "The Agent session belongs to another workspace"
    ),
    (
        "AGENT_WORKSPACE_REQUIRED",
        AgentWorkspaceRequired,
        Agent,
        false,
        ReselectProject,
        "Associate a project before Agent delivery"
    ),
    (
        "AGENT_UNAUTHORIZED",
        AgentUnauthorized,
        Agent,
        false,
        ReadDiagnostics,
        "The Agent request was not authorized"
    ),
    (
        "PAIR_REQUIRED",
        PairRequired,
        Agent,
        false,
        None,
        "No active pairing authorizes this Agent operation"
    ),
    (
        "ASSISTED_CANCELLED",
        AssistedCancelled,
        Agent,
        false,
        AwaitCurrentOperation,
        "The assisted operation was cancelled; confirm again before retrying"
    ),
    (
        "ASSISTED_REPLY_INVALID",
        AssistedReplyInvalid,
        Agent,
        false,
        ReadDiagnostics,
        "The assisted operation produced an unexpected reply"
    ),
    (
        "ASSISTED_REQUEST_PENDING",
        AssistedRequestPending,
        Agent,
        false,
        AwaitCurrentOperation,
        "An assisted operation is already running"
    ),
    (
        "ASSISTED_SCOPE_CHANGED_RECONFIRM_REQUIRED",
        AssistedScopeChangedReconfirmRequired,
        Agent,
        false,
        RefreshSnapshot,
        "The scope changed; inspect the window again before filling"
    ),
    (
        "ASSISTED_TIMEOUT_OR_CANCELLED",
        AssistedTimeoutOrCancelled,
        Agent,
        false,
        RefreshSnapshot,
        "The assisted operation timed out or was cancelled; verify the result manually"
    ),
    (
        "ASSISTED_UNAVAILABLE",
        AssistedUnavailable,
        Agent,
        false,
        ReadDiagnostics,
        "The assisted automation controller is unavailable"
    ),
    // Everything the contract does not recognise. Details are intentionally dropped.
    (
        "INTERNAL",
        Internal,
        Internal,
        false,
        ReadDiagnostics,
        "An internal ArkDock error occurred; review the runtime diagnostics"
    ),
];

/// The canonical spec for a wire code, or `None` when the code is not part of the contract.
pub(crate) fn spec_for(wire: &str) -> Option<&'static ErrorSpec> {
    if wire.is_empty() || wire.len() > MAX_CODE_CHARS || !is_code_token(wire) {
        return None;
    }
    ERROR_TABLE.iter().find(|spec| spec.wire == wire)
}

/// `code` as it appears on the wire. Uses the generated serde renames so the Rust
/// vocabulary and the protocol vocabulary cannot disagree.
pub fn wire_name(code: &ErrorCode) -> String {
    serde_json::to_value(code)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| "INTERNAL".to_owned())
}

/// Uppercase, digits and underscores only; this is what keeps a prose message that
/// merely contains a colon from being read as a machine code.
fn is_code_token(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

impl IpcError {
    /// Classifies an internal `Result<T, String>` failure at the command boundary.
    pub fn from_message(message: impl Into<String>) -> Self {
        let raw = message.into();
        let (wire, residual) = match raw.split_once(':') {
            Some((head, rest)) => (head.trim(), rest.trim()),
            None => (raw.trim(), ""),
        };
        match spec_for(wire) {
            Some(spec) => Self::build(spec, residual),
            // An unclassified failure never echoes the original text: it may contain
            // paths, tokens, or a foreign window title. Diagnostics keep the raw text.
            None => Self::internal(),
        }
    }

    /// The single fallback error. Never carries details.
    pub fn internal() -> Self {
        match spec_for("INTERNAL") {
            Some(spec) => Self::build(spec, ""),
            // Unreachable: the table always declares INTERNAL, enforced by its test.
            None => Self {
                code: ErrorCode::Internal,
                category: ErrorCategory::Internal,
                message: "An internal ArkDock error occurred".to_owned(),
                retryable: false,
                recovery: RecoveryAction::ReadDiagnostics,
                details: None,
            },
        }
    }

    /// Explicit constructor for errors detected directly at the boundary.
    pub fn new(spec_code: ErrorCode, details: Option<String>) -> Self {
        let wire = wire_name(&spec_code);
        match spec_for(&wire) {
            Some(spec) => Self::build(spec, details.as_deref().unwrap_or_default()),
            None => Self::internal(),
        }
    }

    fn build(spec: &'static ErrorSpec, residual: &str) -> Self {
        Self {
            code: spec.code.clone(),
            category: spec.category.clone(),
            message: spec.message.to_owned(),
            retryable: spec.retryable,
            recovery: spec.recovery.clone(),
            // The fallback describes an unclassified failure; it never carries the
            // original text in any form.
            details: if matches!(spec.code, ErrorCode::Internal) {
                None
            } else {
                sanitize_details(residual)
            },
        }
    }
}

impl From<String> for IpcError {
    fn from(message: String) -> Self {
        Self::from_message(message)
    }
}

impl From<&str> for IpcError {
    fn from(message: &str) -> Self {
        Self::from_message(message)
    }
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for IpcError {}

/// Diagnostics keep only bounded, path-redacted context. Absolute Windows, UNC and
/// POSIX tokens are replaced before the text leaves the backend.
fn sanitize_details(value: &str) -> Option<String> {
    let cleaned: String = value
        .chars()
        .filter(|character| !character.is_control())
        .collect();
    let redacted = cleaned
        .split_whitespace()
        .map(|token| {
            if is_absolute_token(token) {
                "[path]"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if redacted.is_empty() {
        return None;
    }
    if redacted.chars().count() <= MAX_DETAILS_CHARS {
        return Some(redacted);
    }
    let mut bounded: String = redacted.chars().take(MAX_DETAILS_CHARS).collect();
    bounded.push('…');
    Some(bounded)
}

fn is_absolute_token(token: &str) -> bool {
    let trimmed = token.trim_start_matches(['(', '"', '\'', '[', '{', '<', '=']);
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
    {
        return true;
    }
    if trimmed.starts_with("\\\\") {
        return true;
    }
    trimmed.starts_with('/')
        && trimmed[1..].starts_with(|character: char| character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn protocol() -> serde_json::Value {
        serde_json::from_str(include_str!("../../../protocol/model.json"))
            .expect("protocol/model.json must stay valid JSON")
    }

    #[test]
    fn error_vocabulary_matches_the_protocol_model() {
        let declared: BTreeSet<String> = protocol()["enums"]["ErrorCode"]
            .as_array()
            .expect("ErrorCode enum")
            .iter()
            .map(|value| value.as_str().expect("string code").to_owned())
            .collect();
        let mapped: BTreeSet<String> = ERROR_TABLE
            .iter()
            .map(|spec| spec.wire.to_owned())
            .collect();
        assert_eq!(
            declared, mapped,
            "protocol/model.json and the backend error table must declare the same codes"
        );
        assert!(
            declared.contains("INTERNAL"),
            "INTERNAL must stay reachable"
        );
    }

    #[test]
    fn table_metadata_uses_declared_categories_and_recoveries() {
        let model = protocol();
        let categories: BTreeSet<String> = model["enums"]["ErrorCategory"]
            .as_array()
            .expect("ErrorCategory enum")
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        let recoveries: BTreeSet<String> = model["enums"]["RecoveryAction"]
            .as_array()
            .expect("RecoveryAction enum")
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .collect();
        for spec in ERROR_TABLE {
            let category = wire_value(&spec.category);
            let recovery = wire_value(&spec.recovery);
            assert!(
                categories.contains(&category),
                "unknown category {category}"
            );
            assert!(
                recoveries.contains(&recovery),
                "unknown recovery {recovery}"
            );
            assert!(!spec.message.is_empty(), "{} needs a message", spec.wire);
        }
    }

    fn wire_value<T: serde::Serialize>(value: &T) -> String {
        serde_json::to_value(value)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .expect("enum value is a string")
    }

    #[test]
    fn every_declared_code_keeps_its_identity_and_semantics() {
        for spec in ERROR_TABLE {
            let error = IpcError::from(format!("{}: something went wrong", spec.wire));
            assert_eq!(error.code, spec.code, "{} lost its code", spec.wire);
            assert_eq!(
                error.category, spec.category,
                "{} lost its category",
                spec.wire
            );
            assert_eq!(
                error.retryable, spec.retryable,
                "{} lost retryable",
                spec.wire
            );
            assert_eq!(error.recovery, spec.recovery, "{} lost recovery", spec.wire);
            assert_eq!(error.message, spec.message);
            assert_eq!(wire_name(&error.code), spec.wire);
            if spec.wire == "INTERNAL" {
                // The fallback itself never carries details, even when one is offered.
                assert!(error.details.is_none());
            } else {
                assert_eq!(error.details.as_deref(), Some("something went wrong"));
            }
        }
    }

    #[test]
    fn the_same_failure_has_the_same_meaning_on_every_path() {
        // Every caller spelling of the same code must produce the same meaning.
        let variants = [
            "STALE_SESSION",
            "STALE_SESSION: an explicit suffix",
            "  STALE_SESSION : padded suffix  ",
        ];
        let mut seen = Vec::new();
        for raw in variants {
            let error = IpcError::from_message(raw);
            seen.push((
                error.code.clone(),
                error.category.clone(),
                error.recovery.clone(),
                error.retryable,
            ));
        }
        assert!(seen.windows(2).all(|pair| pair[0] == pair[1]));
        // And the semantic families the product distinguishes stay distinguishable.
        let codes = [
            "STALE_SESSION",
            "TRUST_REQUIRED",
            "SDK_MISSING",
            "UNSUPPORTED_PLATFORM",
            "BUSY",
            "PREVIEW_FAILED",
        ];
        let mapped: Vec<(ErrorCode, ErrorCategory)> = codes
            .iter()
            .map(|code| {
                let error = IpcError::from_message(*code);
                (error.code, error.category)
            })
            .collect();
        let unique: BTreeSet<String> = mapped.iter().map(|(code, _)| wire_name(code)).collect();
        assert_eq!(
            unique.len(),
            codes.len(),
            "each semantic case needs its own code"
        );
        assert_ne!(mapped[0].1, mapped[1].1);
        assert_ne!(mapped[3].1, mapped[5].1);
    }

    #[test]
    fn unknown_failures_degrade_to_internal_without_leaking_the_message() {
        let error =
            IpcError::from_message(r"the pipe broke while reading C:\Users\someone\secret\app.log");
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(error.category, ErrorCategory::Internal);
        assert!(!error.retryable);
        assert!(error.details.is_none());
        let serialized = serde_json::to_string(&error).unwrap();
        assert!(!serialized.contains("secret"), "{serialized}");
        assert!(!serialized.contains("pipe broke"));
        // A prose message that merely contains a colon is not mistaken for a code.
        let prose = IpcError::from_message("window not found: label main");
        assert_eq!(prose.code, ErrorCode::Internal);
    }

    #[test]
    fn details_are_redacted_and_bounded() {
        let error = IpcError::from_message(
            r"SDK_MISSING: probe failed at C:\Program Files\Huawei\Sdk and \\server\share\sdk and /home/user/sdk",
        );
        let details = error.details.clone().expect("known code keeps context");
        assert!(!details.contains(r"C:\Program Files"), "{details}");
        assert!(!details.contains(r"\\server"), "{details}");
        assert!(!details.contains("/home/user"), "{details}");
        assert_eq!(details.matches("[path]").count(), 3, "{details}");
        let long = IpcError::from_message(format!("SDK_MISSING: {}", "x".repeat(2000)));
        let bounded = long.details.expect("bounded details");
        assert!(bounded.chars().count() <= MAX_DETAILS_CHARS + 1);
        assert!(bounded.ends_with('…'));
        assert!(IpcError::from_message("SDK_MISSING").details.is_none());
    }

    #[test]
    fn non_idempotent_operations_are_never_retryable() {
        for spec in ERROR_TABLE {
            if matches!(spec.category, ErrorCategory::Agent) {
                assert!(!spec.retryable, "{} must not be retryable", spec.wire);
                assert_ne!(spec.recovery, RecoveryAction::RetryRequest, "{}", spec.wire);
            }
            if spec.retryable {
                assert_ne!(
                    spec.recovery,
                    RecoveryAction::ConfirmTrust,
                    "{} must not suggest confirming trust automatically",
                    spec.wire
                );
            }
        }
        // The request-level timeout can be reported after the operation already
        // started, so it must never invite an automatic retry.
        for wire in [
            "REQUEST_TIMEOUT",
            "DATA_REQUEST_TIMEOUT",
            "PAIRING_WRITE_TIMEOUT",
        ] {
            let spec = spec_for(wire).expect("declared");
            assert!(!spec.retryable, "{wire}");
        }
    }
}

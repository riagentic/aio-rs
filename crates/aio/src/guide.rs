//! The guide's pages (`docs/`), so every Rust example in them compiles,
//! and runs unless marked `no_run`, as a doctest of this crate.
//! `scripts/docs-check` fails when a page with Rust in it is missing here.

macro_rules! pages {
    ($($(#[$m:meta])* $name:ident = $path:literal),* $(,)?) => {
        $( $(#[$m])* #[doc = include_str!(concat!("../../../docs/", $path))] pub struct $name; )*
    };
}

pages! {
    Readme = "../README.md",
    Quickstart = "basics/quickstart.md",
    #[cfg(feature = "kit")]
    Tutorial = "basics/tutorial.md",
    Concepts = "basics/concepts.md",
    Project = "basics/project.md",
    #[cfg(feature = "server")]
    Architecture = "basics/architecture.md",
    #[cfg(all(feature = "kit", feature = "server"))]
    Cookbook = "basics/cookbook.md",
    Versioning = "basics/versioning.md",
    Faq = "basics/faq.md",
    ApiMap = "reference/api.md",
    Cells = "state/cells.md",
    Effects = "state/effects.md",
    Methods = "state/methods.md",
    Lifecycle = "state/lifecycle.md",
    Composition = "state/composition.md",
    Scheduling = "state/scheduling.md",
    #[cfg(feature = "server")]
    Visibility = "state/visibility.md",
    #[cfg(feature = "server")]
    Plugins = "state/plugins.md",
    #[cfg(feature = "server")]
    Sync = "state/sync.md",
    Persistence = "persistence/README.md",
    #[cfg(feature = "server")]
    PersistenceSql = "persistence/sql.md",
    PersistenceFiles = "persistence/files.md",
    #[cfg(feature = "server")]
    Offline = "persistence/offline.md",
    #[cfg(feature = "kit")]
    Views = "ui/views.md",
    #[cfg(feature = "kit")]
    Kit = "ui/kit.md",
    #[cfg(feature = "kit")]
    Routing = "ui/routing.md",
    #[cfg(feature = "kit")]
    Theming = "ui/theming.md",
    #[cfg(feature = "kit")]
    Forms = "ui/forms.md",
    #[cfg(feature = "kit")]
    Animation = "ui/animation.md",
    #[cfg(feature = "kit")]
    Islands = "ui/islands.md",
    Release = "build/release.md",
    Targets = "build/targets.md",
    Build = "build/README.md",
    #[cfg(feature = "kit")]
    Dev = "build/dev.md",
    Ci = "build/ci.md",
    Scaling = "build/scaling.md",
    #[cfg(all(feature = "kit", feature = "server"))]
    Testing = "testing/README.md",
    #[cfg(feature = "server")]
    TestingCells = "testing/cells.md",
    #[cfg(all(feature = "kit", feature = "server"))]
    TestingUi = "testing/ui.md",
    TestingCassettes = "testing/cassettes.md",
    #[cfg(feature = "server")]
    TestingMultiApp = "testing/multi-app.md",
    TestingLint = "testing/lint.md",
    Debugging = "debugging/README.md",
    Errors = "debugging/errors.md",
    #[cfg(feature = "kit")]
    TimeTravel = "debugging/time-travel.md",
    Production = "debugging/production.md",
    Feedback = "debugging/feedback.md",
    CliApps = "clients/cli-apps.md",
    #[cfg(feature = "kit")]
    Desktop = "clients/desktop.md",
    #[cfg(feature = "kit")]
    Web = "clients/web.md",
    #[cfg(feature = "server")]
    Transports = "clients/transports.md",
    #[cfg(feature = "server")]
    Auth = "auth/README.md",
    #[cfg(feature = "server")]
    Secrets = "auth/secrets.md",
    #[cfg(feature = "kit")]
    Performance = "performance.md",
    #[cfg(feature = "kit")]
    Migration = "migration/README.md",
    MigrationData = "migration/data.md",
    #[cfg(feature = "server")]
    MigrationWire = "migration/wire.md",
    #[cfg(feature = "kit")]
    ExWorkflow = "examples/workflow.md",
    #[cfg(feature = "server")]
    ExCliService = "examples/cli-service.md",
    #[cfg(all(feature = "kit", feature = "server"))]
    ExDashboard = "examples/dashboard.md",
    #[cfg(feature = "oidc")]
    ExIntegrations = "examples/integrations.md",
}

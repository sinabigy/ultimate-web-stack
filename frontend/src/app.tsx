import { Navigate, Route, Router, type RouteSectionProps } from "@solidjs/router";
import { type Component, ErrorBoundary, lazy, Suspense } from "solid-js";
import { RequireAuth, RequireSystem, SessionProvider } from "./auth/session";
import { Toasts } from "./components/toasts";
import { ErrorState, Skeleton } from "./components/ui";
import { AppShell } from "./features/shell/AppShell";

// Route groups are split into separate chunks: the login page never downloads admin code.
const page = <M extends Record<string, unknown>>(loader: () => Promise<M>, name: keyof M) =>
  lazy(async () => ({ default: (await loader())[name] as Component }));
const auth = () => import("./routes/auth");
const account = () => import("./routes/account");
const org = () => import("./routes/org");
const admin = () => import("./routes/admin");

function Root(props: RouteSectionProps) {
  return (
    <SessionProvider>
      <ErrorBoundary fallback={(err, reset) => <ErrorState error={err} retry={reset} />}>
        {props.children}
      </ErrorBoundary>
      <Toasts />
    </SessionProvider>
  );
}

function Protected(props: RouteSectionProps) {
  return (
    <RequireAuth>
      <AppShell>
        <ErrorBoundary fallback={(err, reset) => <ErrorState error={err} retry={reset} />}>
          <Suspense fallback={<Skeleton lines={6} height="18px" />}>{props.children}</Suspense>
        </ErrorBoundary>
      </AppShell>
    </RequireAuth>
  );
}

const AccountLayout = page(account, "AccountLayout");
const OrgLayout = page(org, "OrgLayout");
const AdminGuard = (props: RouteSectionProps) => <RequireSystem>{props.children}</RequireSystem>;

export function App() {
  return (
    <Router root={Root}>
      <Route path="/" component={() => <Navigate href="/dashboard" />} />
      <Route path="/login" component={page(auth, "LoginPage")} />
      <Route path="/register" component={page(auth, "RegisterPage")} />
      <Route path="/forgot-password" component={page(auth, "ForgotPasswordPage")} />
      <Route path="/reset-password" component={page(auth, "ResetPasswordPage")} />
      <Route path="/verify-email" component={page(auth, "VerifyEmailPage")} />
      <Route path="/auth/callback" component={page(auth, "AuthCallbackPage")} />
      <Route path="/logout" component={page(auth, "LogoutPage")} />
      <Route path="/" component={Protected}>
        <Route path="/invitations/:token" component={page(auth, "InvitationPage")} />
        <Route
          path="/dashboard"
          component={lazy(() => import("./routes/dashboard").then((m) => ({ default: m.DashboardPage })))}
        />
        <Route path="/account" component={AccountLayout}>
          <Route path="/" component={() => <Navigate href="/account/profile" />} />
          <Route path="/profile" component={page(account, "ProfilePage")} />
          <Route path="/security" component={page(account, "SecurityPage")} />
          <Route path="/passkeys" component={page(account, "PasskeysPage")} />
          <Route path="/mfa" component={page(account, "MfaPage")} />
          <Route path="/sessions" component={page(account, "SessionsPage")} />
          <Route path="/preferences" component={page(account, "PreferencesPage")} />
          <Route path="/notifications" component={page(account, "NotificationsPage")} />
          <Route path="/activity" component={page(account, "ActivityPage")} />
        </Route>
        <Route path="/org/:slug" component={OrgLayout}>
          <Route path="/" component={page(org, "OrgOverviewPage")} />
          <Route path="/runs" component={page(org, "RunsPage")} />
          <Route path="/members" component={page(org, "MembersPage")} />
          <Route path="/teams" component={page(org, "TeamsPage")} />
          <Route path="/roles" component={page(org, "RolesPage")} />
          <Route path="/settings" component={page(org, "OrgSettingsPage")} />
          <Route path="/audit" component={page(org, "OrgAuditPage")} />
          <Route path="/api-keys" component={page(org, "ApiKeysPage")} />
          <Route path="/billing" component={page(org, "BillingPage")} />
        </Route>
        <Route path="/admin" component={AdminGuard}>
          <Route path="/" component={page(admin, "AdminOverviewPage")} />
          <Route path="/users" component={page(admin, "AdminUsersPage")} />
          <Route path="/users/:id" component={page(admin, "AdminUserDetailPage")} />
          <Route path="/organizations" component={page(admin, "AdminOrgsPage")} />
          <Route path="/roles" component={page(admin, "AdminRolesPage")} />
          <Route path="/audit" component={page(admin, "AdminAuditPage")} />
          <Route path="/jobs" component={page(admin, "AdminJobsPage")} />
          <Route path="/providers" component={page(admin, "AdminProvidersPage")} />
          <Route path="/system" component={page(admin, "AdminSystemPage")} />
        </Route>
      </Route>
      <Route
        path="*404"
        component={lazy(() => import("./routes/notfound").then((m) => ({ default: m.NotFoundPage })))}
      />
    </Router>
  );
}

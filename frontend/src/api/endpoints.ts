// Typed API surface. Every type comes from the Rust backend (src/api/generated, via ts-rs).
import { http, qs } from "./client";
import type { AdminJobs } from "./generated/AdminJobs";
import type { AdminOrgRow } from "./generated/AdminOrgRow";
import type { AdminOverview } from "./generated/AdminOverview";
import type { AdminUserDetail } from "./generated/AdminUserDetail";
import type { AdminUserRow } from "./generated/AdminUserRow";
import type { ApiKeyCreated } from "./generated/ApiKeyCreated";
import type { ApiKeyRotated } from "./generated/ApiKeyRotated";
import type { ApiKeyRow } from "./generated/ApiKeyRow";
import type { AuditRow } from "./generated/AuditRow";
import type { BillingResponse } from "./generated/BillingResponse";
import type { CountResponse } from "./generated/CountResponse";
import type { Created } from "./generated/Created";
import type { DashboardResponse } from "./generated/DashboardResponse";
import type { InvitationAccepted } from "./generated/InvitationAccepted";
import type { InvitationRow } from "./generated/InvitationRow";
import type { InvitationView } from "./generated/InvitationView";
import type { InviteCreated } from "./generated/InviteCreated";
import type { ListResponse } from "./generated/ListResponse";
import type { LogoutResponse } from "./generated/LogoutResponse";
import type { MemberRow } from "./generated/MemberRow";
import type { MyOrg } from "./generated/MyOrg";
import type { NotificationsResponse } from "./generated/NotificationsResponse";
import type { OkResponse } from "./generated/OkResponse";
import type { OrgDetail } from "./generated/OrgDetail";
import type { OrgOverview } from "./generated/OrgOverview";
import type { OrgRow } from "./generated/OrgRow";
import type { Page } from "./generated/Page";
import type { PagedResponse } from "./generated/PagedResponse";
import type { PermissionInfo } from "./generated/PermissionInfo";
import type { ProfileResponse } from "./generated/ProfileResponse";
import type { ProfileUpdated } from "./generated/ProfileUpdated";
import type { ProviderInfo } from "./generated/ProviderInfo";
import type { RoleRow } from "./generated/RoleRow";
import type { RolesModelResponse } from "./generated/RolesModelResponse";
import type { Run } from "./generated/Run";
import type { RunAnalytics } from "./generated/RunAnalytics";
import type { SecurityResponse } from "./generated/SecurityResponse";
import type { SessionItem } from "./generated/SessionItem";
import type { SessionResponse } from "./generated/SessionResponse";
import type { SystemInfo } from "./generated/SystemInfo";
import type { TeamRow } from "./generated/TeamRow";

const o = (slug: string) => `/api/v1/orgs/${encodeURIComponent(slug)}`;

export interface AuditQuery {
  action?: string;
  outcome?: string;
  cursor?: string;
  limit?: number;
  since?: number;
  until?: number;
}

export const api = {
  session: () => http.get<SessionResponse>("/api/v1/session", { quiet401: true }),
  logout: () => http.post<LogoutResponse>("/auth/logout"),
  dashboard: () => http.get<DashboardResponse>("/api/v1/dashboard"),

  account: {
    profile: () => http.get<ProfileResponse>("/api/v1/account/profile"),
    updateProfile: (b: { display_name: string; avatar_url?: string | null }) =>
      http.patch<ProfileUpdated>("/api/v1/account/profile", b),
    updatePreferences: (prefs: Record<string, unknown>) =>
      http.put<void>("/api/v1/account/preferences", prefs),
    security: () => http.get<SecurityResponse>("/api/v1/account/security"),
    removeMethod: (kind: string, id: string) =>
      http.del<void>(
        `/api/v1/account/security/methods/${encodeURIComponent(kind)}/${encodeURIComponent(id)}`,
      ),
    resendVerification: () => http.post<void>("/api/v1/account/security/verify-email"),
    sessions: () => http.get<ListResponse<SessionItem>>("/api/v1/account/sessions"),
    revokeSession: (id: string) => http.del<void>(`/api/v1/account/sessions/${id}`),
    revokeAll: (include_current = false) =>
      http.post<CountResponse>("/api/v1/account/sessions/revoke-all", { include_current }),
    activity: () => http.get<Page<AuditRow>>("/api/v1/account/activity"),
    deleteAccount: (confirm_email: string) => http.post<void>("/api/v1/account/delete", { confirm_email }),
  },

  notifications: {
    list: (unread = false) =>
      http.get<NotificationsResponse>(`/api/v1/notifications${qs({ unread: unread || undefined })}`),
    read: (id: string) => http.post<void>(`/api/v1/notifications/${id}/read`),
    readAll: () => http.post<CountResponse>("/api/v1/notifications/read-all"),
  },

  permissions: () => http.get<ListResponse<PermissionInfo>>("/api/v1/permissions"),
  invitation: (token: string) => http.get<InvitationView>(`/api/v1/invitations/${encodeURIComponent(token)}`),
  acceptInvitation: (token: string) =>
    http.post<InvitationAccepted>(`/api/v1/invitations/${encodeURIComponent(token)}/accept`),

  orgs: {
    list: () => http.get<ListResponse<MyOrg>>("/api/v1/orgs"),
    create: (b: { name: string; slug?: string }) => http.post<OrgRow>("/api/v1/orgs", b),
    get: (slug: string) => http.get<OrgDetail>(o(slug)),
    update: (slug: string, b: { name?: string; settings?: Record<string, unknown> }) =>
      http.patch<OrgRow>(o(slug), b),
    remove: (slug: string) => http.del<void>(o(slug)),
    overview: (slug: string, days = 14) => http.get<OrgOverview>(`${o(slug)}/overview${qs({ days })}`),
    /** ClickHouse-backed run analytics; 404 when the analytics module is off. */
    runAnalytics: (slug: string, days = 30) =>
      http.get<RunAnalytics>(`${o(slug)}/analytics/runs${qs({ days })}`),
    leave: (slug: string) => http.post<void>(`${o(slug)}/leave`),
    members: (slug: string) => http.get<ListResponse<MemberRow>>(`${o(slug)}/members`),
    changeRole: (slug: string, userId: string, role_id: string) =>
      http.patch<void>(`${o(slug)}/members/${userId}`, { role_id }),
    removeMember: (slug: string, userId: string) => http.del<void>(`${o(slug)}/members/${userId}`),
    invitations: (slug: string) => http.get<ListResponse<InvitationRow>>(`${o(slug)}/invitations`),
    invite: (slug: string, b: { email: string; role_id: string }) =>
      http.post<InviteCreated>(`${o(slug)}/invitations`, b),
    revokeInvitation: (slug: string, id: string) => http.del<void>(`${o(slug)}/invitations/${id}`),
    teams: (slug: string) => http.get<ListResponse<TeamRow>>(`${o(slug)}/teams`),
    createTeam: (slug: string, b: { name: string; description?: string }) =>
      http.post<Created>(`${o(slug)}/teams`, b),
    deleteTeam: (slug: string, id: string) => http.del<void>(`${o(slug)}/teams/${id}`),
    roles: (slug: string) => http.get<ListResponse<RoleRow>>(`${o(slug)}/roles`),
    createRole: (
      slug: string,
      b: { key: string; name: string; description?: string; permissions: string[] },
    ) => http.post<Created>(`${o(slug)}/roles`, b),
    deleteRole: (slug: string, id: string) => http.del<void>(`${o(slug)}/roles/${id}`),
    audit: (slug: string, q: AuditQuery = {}) => http.get<Page<AuditRow>>(`${o(slug)}/audit${qs({ ...q })}`),
    apiKeys: (slug: string) => http.get<ListResponse<ApiKeyRow>>(`${o(slug)}/api-keys`),
    createApiKey: (slug: string, b: { name: string; scopes: string[]; expires_in_days?: number }) =>
      http.post<ApiKeyCreated>(`${o(slug)}/api-keys`, b),
    rotateApiKey: (slug: string, id: string) => http.post<ApiKeyRotated>(`${o(slug)}/api-keys/${id}/rotate`),
    revokeApiKey: (slug: string, id: string) => http.del<void>(`${o(slug)}/api-keys/${id}`),
    billing: (slug: string) => http.get<BillingResponse>(`${o(slug)}/billing`),
    runs: (slug: string, q: { status?: string; cursor?: string; limit?: number } = {}) =>
      http.get<Page<Run>>(`${o(slug)}/runs${qs({ ...q })}`),
    createRun: (slug: string, b: { label: string; provider: string; requested: number }) =>
      http.post<Run>(`${o(slug)}/runs`, b),
    run: (slug: string, id: string) => http.get<Run>(`${o(slug)}/runs/${id}`),
    deleteRun: (slug: string, id: string) => http.del<void>(`${o(slug)}/runs/${id}`),
  },

  admin: {
    overview: () => http.get<AdminOverview>("/api/v1/admin/overview"),
    users: (q: { search?: string; status?: string; page?: number; per_page?: number }) =>
      http.get<PagedResponse<AdminUserRow>>(`/api/v1/admin/users${qs({ ...q })}`),
    user: (id: string) => http.get<AdminUserDetail>(`/api/v1/admin/users/${id}`),
    updateUser: (id: string, b: { status?: string; system_role?: string }) =>
      http.patch<OkResponse>(`/api/v1/admin/users/${id}`, b),
    revokeUserSessions: (id: string) => http.post<CountResponse>(`/api/v1/admin/users/${id}/revoke-sessions`),
    orgs: (q: { search?: string; page?: number; per_page?: number }) =>
      http.get<PagedResponse<AdminOrgRow>>(`/api/v1/admin/organizations${qs({ ...q })}`),
    roles: () => http.get<RolesModelResponse>("/api/v1/admin/roles"),
    audit: (q: AuditQuery = {}) => http.get<Page<AuditRow>>(`/api/v1/admin/audit${qs({ ...q })}`),
    jobs: (status?: string) => http.get<AdminJobs>(`/api/v1/admin/jobs${qs({ status })}`),
    retryJob: (id: string) => http.post<void>(`/api/v1/admin/jobs/${id}/retry`),
    providers: () => http.get<ListResponse<ProviderInfo>>("/api/v1/admin/providers"),
    system: () => http.get<SystemInfo>("/api/v1/admin/system"),
  },
};

/** Browser navigations to the BFF login endpoints (never fetch: they redirect to the IdP). */
export const authUrls = {
  login: (p: { returnTo?: string; method?: string; loginHint?: string } = {}) =>
    `/auth/login${qs({ return_to: p.returnTo, method: p.method, login_hint: p.loginHint })}`,
  register: (p: { returnTo?: string; loginHint?: string } = {}) =>
    `/auth/register${qs({ return_to: p.returnTo, login_hint: p.loginHint })}`,
  reauth: (returnTo?: string) => `/auth/reauth${qs({ return_to: returnTo })}`,
};

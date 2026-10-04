import { A } from "@solidjs/router";
import { EmptyState } from "../components/ui";

export function NotFoundPage() {
  return (
    <main class="auth-page">
      <EmptyState
        icon="alert"
        title="Page not found"
        body="The page you're looking for doesn't exist or you don't have access."
        action={
          <A class="btn btn-primary" href="/dashboard">
            Go to dashboard
          </A>
        }
      />
    </main>
  );
}

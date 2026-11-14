import { Link } from '@tanstack/react-router';
import { EmptyState } from '@/components/ui/feedback';

export function NotFoundPage() {
  return (
    <div className="flex h-full items-center justify-center">
      <EmptyState
        title="Page not found"
        description="The page you are looking for does not exist or was moved."
        action={
          <Link
            to="/sessions"
            className="text-[12px] font-bold text-primary hover:text-primary-hover"
          >
            Back to sessions
          </Link>
        }
      />
    </div>
  );
}

export function RouteError({ error, reset }: { error: unknown; reset?: () => void }) {
  const message = error instanceof Error ? error.message : String(error);
  return (
    <div className="flex h-full items-center justify-center">
      <EmptyState
        title="Something broke"
        description={message}
        action={
          reset ? (
            <button
              type="button"
              onClick={reset}
              className="h-8 rounded-md border border-border bg-card px-3 text-[11.5px] font-bold text-foreground shadow-[0_1px_2px_#0000000a] hover:bg-muted"
            >
              Try again
            </button>
          ) : null
        }
      />
    </div>
  );
}

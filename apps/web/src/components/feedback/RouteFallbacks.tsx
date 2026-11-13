import { Button, Card, EmptyState } from '@agent-fleet/ui';
import { Link } from '@tanstack/react-router';

export function NotFoundPage() {
  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <Card className="w-full max-w-xl">
        <EmptyState
          title="Page not found"
          description="The URL does not match a route in the operations console."
          action={
            <Link
              to="/machines"
              className="inline-flex h-8 items-center justify-center rounded-md border border-slate-700 bg-slate-900 px-3 text-xs font-medium text-slate-200 hover:border-slate-600 hover:bg-slate-800"
            >
              Back to machines
            </Link>
          }
        />
      </Card>
    </div>
  );
}

export function RouteError() {
  return (
    <div className="flex min-h-[60vh] items-center justify-center">
      <Card className="w-full max-w-xl">
        <EmptyState
          title="The console hit an unexpected error"
          description="Reload the page to recover. Server data and live sessions remain on the backend."
          action={
            <Button variant="secondary" size="sm" onClick={() => window.location.reload()}>
              Reload console
            </Button>
          }
        />
      </Card>
    </div>
  );
}

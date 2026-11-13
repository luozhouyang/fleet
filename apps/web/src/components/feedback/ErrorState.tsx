import { Button, Card, EmptyState } from '@agent-fleet/ui';
import { getErrorMessage } from '@/lib/utils/format';

export function ErrorState({
  error,
  title = 'Could not load this view',
  onRetry,
}: {
  error: unknown;
  title?: string;
  onRetry?: () => void;
}) {
  return (
    <Card>
      <EmptyState
        title={title}
        description={getErrorMessage(error)}
        icon={
          <svg viewBox="0 0 24 24" fill="none" className="size-5" aria-hidden="true">
            <path
              d="M12 8v5m0 3.5v.5M10.3 4.1 2.8 17.2A2 2 0 0 0 4.5 20h15a2 2 0 0 0 1.7-2.8L13.7 4.1a2 2 0 0 0-3.4 0Z"
              stroke="currentColor"
              strokeWidth="1.6"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        }
        action={
          onRetry ? (
            <Button variant="secondary" size="sm" onClick={onRetry}>
              Try again
            </Button>
          ) : undefined
        }
      />
    </Card>
  );
}

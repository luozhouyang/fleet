import { Card, Spinner } from '@agent-fleet/ui';

export function LoadingState({ label = 'Loading…' }: { label?: string }) {
  return (
    <Card className="flex min-h-48 items-center justify-center">
      <div className="flex items-center gap-3 text-sm text-slate-400">
        <Spinner size="sm" label={label} />
        {label}
      </div>
    </Card>
  );
}

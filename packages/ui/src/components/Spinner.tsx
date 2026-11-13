import { cn } from '../cn';

export function Spinner({
  size = 'md',
  className,
  label = 'Loading',
}: {
  size?: 'sm' | 'md' | 'lg';
  className?: string;
  label?: string;
}) {
  const dimensions = {
    sm: 'size-4 border-2',
    md: 'size-6 border-2',
    lg: 'size-8 border-[3px]',
  }[size];

  return (
    <span
      role="status"
      aria-label={label}
      className={cn(
        'inline-block animate-spin rounded-full border-teal-400/80 border-r-transparent',
        dimensions,
        className,
      )}
    />
  );
}

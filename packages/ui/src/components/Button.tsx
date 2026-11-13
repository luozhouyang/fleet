import type { ButtonHTMLAttributes, ReactNode } from 'react';
import { cn } from '../cn';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
export type ButtonSize = 'sm' | 'md' | 'icon';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  leadingIcon?: ReactNode;
}

const variants: Record<ButtonVariant, string> = {
  primary:
    'border border-teal-400/40 bg-teal-500 text-slate-950 shadow-sm shadow-teal-950/30 hover:bg-teal-400',
  secondary:
    'border border-slate-700 bg-slate-900 text-slate-200 hover:border-slate-600 hover:bg-slate-800',
  ghost: 'border border-transparent text-slate-300 hover:bg-slate-800/70 hover:text-slate-100',
  danger:
    'border border-red-500/40 bg-red-500/10 text-red-300 hover:bg-red-500/20 hover:text-red-200',
};

const sizes: Record<ButtonSize, string> = {
  sm: 'h-8 px-3 text-xs',
  md: 'h-9 px-4 text-sm',
  icon: 'size-9 p-0',
};

export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  leadingIcon,
  className,
  children,
  disabled,
  type = 'button',
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      disabled={disabled || loading}
      className={cn(
        'inline-flex items-center justify-center gap-2 rounded-md font-medium transition-colors',
        'focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal-400/70',
        'disabled:cursor-not-allowed disabled:opacity-50',
        variants[variant],
        sizes[size],
        className,
      )}
      {...props}
    >
      {loading ? (
        <span
          aria-hidden="true"
          className="size-3.5 animate-spin rounded-full border-2 border-current border-r-transparent"
        />
      ) : (
        leadingIcon
      )}
      {children}
    </button>
  );
}

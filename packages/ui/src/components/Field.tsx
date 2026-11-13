import type {
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
} from 'react';
import { cn } from '../cn';

export function Field({
  label,
  htmlFor,
  hint,
  error,
  required,
  children,
  className,
}: {
  label: ReactNode;
  htmlFor?: string;
  hint?: ReactNode;
  error?: ReactNode;
  required?: boolean;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn('space-y-1.5', className)}>
      <label
        htmlFor={htmlFor}
        className="flex items-center gap-1 text-xs font-medium text-slate-300"
      >
        {label}
        {required ? <span className="text-teal-400">*</span> : null}
      </label>
      {children}
      {error ? (
        <p className="text-xs leading-5 text-red-400">{error}</p>
      ) : hint ? (
        <p className="text-xs leading-5 text-slate-500">{hint}</p>
      ) : null}
    </div>
  );
}

const controlClasses =
  'w-full rounded-md border border-slate-700 bg-slate-950/70 px-3 text-sm text-slate-100 placeholder:text-slate-600 shadow-inner shadow-black/10 focus:border-teal-500/70 focus:outline-none focus:ring-2 focus:ring-teal-500/20 disabled:cursor-not-allowed disabled:opacity-60';

export function Input({ className, ...props }: InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn(controlClasses, 'h-9', className)} {...props} />;
}

export function Select({ className, ...props }: SelectHTMLAttributes<HTMLSelectElement>) {
  return <select className={cn(controlClasses, 'h-9 pr-8', className)} {...props} />;
}

export function Textarea({ className, ...props }: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(controlClasses, 'min-h-24 resize-y py-2 leading-6', className)}
      {...props}
    />
  );
}

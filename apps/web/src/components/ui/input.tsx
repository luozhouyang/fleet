import * as React from 'react';
import { cn } from '@/lib/utils/cn';

export const inputClassName =
  'h-[34px] w-full rounded-md border border-border bg-card px-2.5 text-[12.5px] text-foreground shadow-[0_1px_2px_#00000008] outline-none transition-[border-color,box-shadow] placeholder:text-faint focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20 disabled:cursor-not-allowed disabled:opacity-50';

export const Input = React.forwardRef<
  HTMLInputElement,
  React.InputHTMLAttributes<HTMLInputElement>
>(({ className, ...props }, ref) => (
  <input ref={ref} className={cn(inputClassName, className)} {...props} />
));
Input.displayName = 'Input';

export const Textarea = React.forwardRef<
  HTMLTextAreaElement,
  React.TextareaHTMLAttributes<HTMLTextAreaElement>
>(({ className, ...props }, ref) => (
  <textarea
    ref={ref}
    className={cn(inputClassName, 'h-auto min-h-[72px] resize-none py-2 leading-5', className)}
    {...props}
  />
));
Textarea.displayName = 'Textarea';

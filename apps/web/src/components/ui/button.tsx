import { cva, type VariantProps } from 'class-variance-authority';
import * as React from 'react';
import { cn } from '@/lib/utils/cn';

export const buttonVariants = cva(
  'inline-flex items-center justify-center gap-1.5 whitespace-nowrap rounded-md text-[12.5px] font-bold transition-[background-color,border-color,color,box-shadow,transform] duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/35 focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:pointer-events-none disabled:opacity-45 active:translate-y-px',
  {
    variants: {
      variant: {
        primary:
          'border border-primary bg-primary text-primary-foreground shadow-[0_1px_2px_#00000014] hover:bg-primary-hover hover:border-primary-hover',
        secondary:
          'border border-border bg-card text-foreground shadow-[0_1px_2px_#0000000a] hover:bg-muted',
        ghost:
          'border border-transparent text-muted-foreground hover:bg-muted hover:text-foreground',
        danger:
          'border border-destructive bg-destructive text-white shadow-[0_1px_2px_#00000014] hover:brightness-95',
      },
      size: {
        sm: 'h-7 px-2.5 text-[11.5px]',
        md: 'h-[34px] px-3.5',
        lg: 'h-9 px-4',
        icon: 'size-[34px] p-0',
        'icon-sm': 'size-7 p-0',
      },
    },
    defaultVariants: {
      variant: 'secondary',
      size: 'md',
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonVariants> {}

export const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, type = 'button', ...props }, ref) => (
    <button
      ref={ref}
      type={type}
      className={cn(buttonVariants({ variant, size }), className)}
      {...props}
    />
  ),
);
Button.displayName = 'Button';

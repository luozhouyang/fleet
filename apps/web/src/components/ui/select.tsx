import { Select as BaseSelect } from '@base-ui-components/react/select';
import { Check, ChevronDown } from 'lucide-react';
import { cn } from '@/lib/utils/cn';

export interface SelectOption {
  value: string;
  label: string;
  description?: string;
  disabled?: boolean;
}

export function Select({
  id,
  value,
  onValueChange,
  options,
  placeholder = 'Select…',
  className,
  disabled,
  'aria-label': ariaLabel,
}: {
  id?: string;
  value: string;
  onValueChange: (value: string) => void;
  options: SelectOption[];
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  'aria-label'?: string;
}) {
  return (
    <BaseSelect.Root
      value={value}
      onValueChange={(next) => onValueChange(next == null ? '' : String(next))}
      disabled={disabled}
    >
      <BaseSelect.Trigger
        id={id}
        aria-label={ariaLabel}
        className={cn(
          'flex h-[34px] w-full items-center justify-between gap-2 rounded-md border border-border bg-card px-2.5 text-left text-[12.5px] text-foreground shadow-[0_1px_2px_#00000008] outline-none transition-[border-color,box-shadow] focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/20 data-[disabled]:cursor-not-allowed data-[disabled]:opacity-50',
          className,
        )}
      >
        <BaseSelect.Value>
          {(selected) => {
            const match = options.find((option) => option.value === selected);
            return match ? (
              <span className="truncate">{match.label}</span>
            ) : (
              <span className="truncate text-faint">{placeholder}</span>
            );
          }}
        </BaseSelect.Value>
        <BaseSelect.Icon>
          <ChevronDown className="size-3.5 shrink-0 text-muted-foreground" />
        </BaseSelect.Icon>
      </BaseSelect.Trigger>
      <BaseSelect.Portal>
        <BaseSelect.Positioner sideOffset={6} className="z-[60]">
          <BaseSelect.Popup className="min-w-44 animate-fade rounded-lg border border-border bg-card p-1 text-body shadow-modal outline-none">
            <BaseSelect.List>
              {options.map((option) => (
                <BaseSelect.Item
                  key={option.value}
                  value={option.value}
                  disabled={option.disabled}
                  className="flex cursor-default items-center justify-between gap-3 rounded-md px-2 py-1.5 text-[12.5px] outline-none data-[disabled]:opacity-40 data-[highlighted]:bg-muted data-[selected]:text-foreground"
                >
                  <span className="flex min-w-0 flex-col">
                    <BaseSelect.ItemText className="truncate">{option.label}</BaseSelect.ItemText>
                    {option.description ? (
                      <span className="truncate font-mono text-[9px] text-faint">
                        {option.description}
                      </span>
                    ) : null}
                  </span>
                  <BaseSelect.ItemIndicator>
                    <Check className="size-3.5 shrink-0 text-primary" />
                  </BaseSelect.ItemIndicator>
                </BaseSelect.Item>
              ))}
            </BaseSelect.List>
          </BaseSelect.Popup>
        </BaseSelect.Positioner>
      </BaseSelect.Portal>
    </BaseSelect.Root>
  );
}

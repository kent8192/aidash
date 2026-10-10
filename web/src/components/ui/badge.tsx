import * as React from "react";
import { cva, type VariantProps } from "class-variance-authority";

import { cn } from "../../lib/utils";

/** Compact label. `tone` carries status meaning; `variant` keeps the shadcn API for neutral uses. */
const badgeVariants = cva(
  "inline-flex h-5 shrink-0 items-center gap-1 whitespace-nowrap rounded-sm border px-1.5 text-[11px] font-medium leading-none",
  {
    variants: {
      variant: {
        default: "border-transparent bg-primary text-primary-foreground",
        secondary: "border-transparent bg-raised text-foreground",
        destructive: "border-transparent bg-destructive-soft text-destructive",
        outline: "border-border-strong text-muted-foreground",
      },
      tone: {
        none: "",
        brand: "border-transparent bg-brand-soft text-brand",
        success: "border-transparent bg-success-soft text-success",
        warning: "border-transparent bg-warning-soft text-warning",
        danger: "border-transparent bg-destructive-soft text-destructive",
        neutral: "border-transparent bg-neutral-soft text-muted-foreground",
      },
    },
    defaultVariants: {
      variant: "outline",
      tone: "none",
    },
  },
);

export interface BadgeProps
  extends React.HTMLAttributes<HTMLSpanElement>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, tone, ...props }: BadgeProps) {
  return (
    <span
      className={cn(badgeVariants({ variant, tone }), className)}
      {...props}
    />
  );
}

export { Badge, badgeVariants };

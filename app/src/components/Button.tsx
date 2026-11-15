import type { ButtonHTMLAttributes, ReactNode } from 'react'

type Variant = 'primary' | 'secondary' | 'ghost' | 'danger'
type Size = 'sm' | 'md' | 'lg'

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant
  size?: Size
  loading?: boolean
  children: ReactNode
}

const variants: Record<Variant, string> = {
  primary: 'bg-accent text-on-accent shadow-glow hover:bg-accent-hover',
  secondary: 'bg-surface-2 text-text border border-border hover:bg-surface',
  ghost: 'bg-transparent text-text-muted hover:bg-surface-2 hover:text-text',
  danger: 'bg-danger text-white hover:opacity-90',
}

const sizes: Record<Size, string> = {
  sm: 'h-7 px-2 text-sm',
  md: 'h-8 px-3 text-base',
  lg: 'h-10 px-4 text-base',
}

/** Primary button primitive (components.md). Loading keeps the label
 *  for screen readers and swaps the icon for a spinner. */
export function Button({
  variant = 'secondary',
  size = 'md',
  loading = false,
  children,
  disabled,
  className = '',
  ...rest
}: ButtonProps) {
  return (
    <button
      type="button"
      aria-busy={loading || undefined}
      disabled={disabled || loading}
      className={`inline-flex items-center justify-center gap-2 rounded-md font-semibold transition-colors disabled:opacity-45 ${variants[variant]} ${sizes[size]} ${className}`}
      {...rest}
    >
      {loading ? (
        <span aria-hidden="true" className="inline-block h-4 w-4 animate-spin rounded-full border-2 border-current border-t-transparent" />
      ) : null}
      {children}
    </button>
  )
}

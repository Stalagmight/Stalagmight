import { type ReactNode, type ButtonHTMLAttributes, forwardRef } from 'react';

type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
type ButtonSize = 'sm' | 'md' | 'lg';

interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  'data-testid'?: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  children: ReactNode;
  icon?: ReactNode;
  /**
   * Tint for `icon`, taken from `MENU_ICON_COLORS` so an action wears the same
   * icon colour here as in the menus. Applied on the unfilled variants only
   * (`secondary`, `ghost`): on a filled `primary` / `danger` button the icon keeps
   * the button's own text colour, which is the one guaranteed to read on that fill.
   */
  iconColor?: string;
}

/** Variants drawn on the panel colour, where a tinted icon reads like it does in a menu. */
const TINTABLE_ICON_VARIANTS: ReadonlySet<ButtonVariant> = new Set(['secondary', 'ghost']);

const variantStyles: Record<ButtonVariant, string> = {
  primary: 'bg-accent text-accent-on hover:bg-accent-90',
  secondary: 'bg-button-secondary border border-border text-text-primary hover:bg-button-secondary-hover',
  ghost: 'text-text-primary hover:bg-panel',
  danger: 'bg-error text-accent-on hover:bg-error-90',
};

const sizeStyles: Record<ButtonSize, string> = {
  sm: 'px-2 py-1 text-sm',
  md: 'px-3 py-1.5',
  lg: 'px-4 py-2 text-lg',
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = 'secondary',
  size = 'md',
  children,
  icon,
  iconColor,
  className = '',
  disabled,
  'data-testid': testId,
  ...props
}, ref) {
  const iconStyle = iconColor && TINTABLE_ICON_VARIANTS.has(variant) ? { color: iconColor } : undefined;
  return (
    <button
      ref={ref}
      className={`
        inline-flex items-center justify-center gap-2 rounded
        font-medium transition-colors
        disabled:opacity-50 disabled:cursor-not-allowed
        ${variantStyles[variant]}
        ${sizeStyles[size]}
        ${className}
      `}
      disabled={disabled}
      data-testid={testId}
      {...(variant === 'primary' ? { 'data-primary-action': '' } : {})}
      {...props}
    >
      {icon && <span className="flex-shrink-0" style={iconStyle} data-icon-color={iconStyle ? iconColor : undefined}>{icon}</span>}
      {children}
    </button>
  );
});

interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  'data-testid'?: string;
  icon: ReactNode;
  variant?: ButtonVariant;
  size?: ButtonSize;
  tooltip?: string;
  active?: boolean;
}

export const IconButton = forwardRef<HTMLButtonElement, IconButtonProps>(function IconButton({
  icon,
  variant = 'ghost',
  size = 'md',
  tooltip,
  active,
  className = '',
  'data-testid': testId,
  ...props
}, ref) {
  const iconSizes: Record<ButtonSize, string> = {
    sm: 'w-7 h-7',
    md: 'w-9 h-9',
    lg: 'w-11 h-11',
  };

  return (
    <button
      ref={ref}
      className={`
        inline-flex items-center justify-center rounded
        transition-colors
        disabled:opacity-50 disabled:cursor-not-allowed
        ${variantStyles[variant]}
        ${iconSizes[size]}
        ${active ? 'ring-2 ring-accent' : ''}
        ${className}
      `}
      title={tooltip}
      data-testid={testId}
      {...props}
    >
      {icon}
    </button>
  );
});

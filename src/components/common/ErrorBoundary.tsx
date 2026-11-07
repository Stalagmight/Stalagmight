import { Component, type ErrorInfo, type ReactNode } from 'react';
// A class component cannot call `useTranslation`, so the fallback reads the shared instance.
import i18n from '../../i18n';

interface ErrorBoundaryProps {
  children: ReactNode;
  panelName: string;
}

interface ErrorBoundaryState {
  hasError: boolean;
  error: Error | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, ErrorBoundaryState> {
  state: ErrorBoundaryState = { hasError: false, error: null };

  static getDerivedStateFromError(error: Error): ErrorBoundaryState {
    return { hasError: true, error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`[ErrorBoundary] ${this.props.panelName} panel error:`, error, info);
  }

  resetError = () => {
    this.setState({ hasError: false, error: null });
  };

  render() {
    if (this.state.hasError) {
      return (
        <div
          className="flex flex-col items-center justify-center h-full gap-3 p-6 bg-surface text-text-secondary"
          data-testid="error-boundary-fallback"
        >
          <p className="text-sm font-medium">
            {i18n.t('errorBoundary.panelError', { panel: this.props.panelName })}
          </p>
          {this.state.error?.message && (
            <p className="text-xs text-text-secondary/70 max-w-xs text-center break-words">
              {this.state.error.message}
            </p>
          )}
          <button
            onClick={this.resetError}
            data-testid="error-boundary-reset"
            className="px-3 py-1.5 text-sm rounded border border-border bg-panel text-text-primary hover:bg-border transition-colors"
          >
            {i18n.t('errorBoundary.tryAgain')}
          </button>
        </div>
      );
    }

    return this.props.children;
  }
}

import { useContext, createContext } from 'react';

export type Theme = {
	mode: 'dark' | 'light' | 'auto';
	current: 'dark' | 'light';
};

export const ThemeContext = createContext<{
	theme: Theme;
	toggleTheme: (theme?: Omit<Theme, 'current'>) => void;
}>({
	theme: {
		mode: 'auto',
		current: 'light'
	},
	toggleTheme: () => {}
});

export function useTheme() {
	return useContext(ThemeContext);
}

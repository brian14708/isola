import { useCallback, useState } from 'react';
import { Theme, ThemeContext } from '@/hooks/theme';

const calcTheme = (theme: Omit<Theme, 'current'>): Theme => {
	let dark = theme.mode === 'dark';
	if (theme.mode === 'auto') {
		dark = window.matchMedia('(prefers-color-scheme: dark)').matches;
	}
	const t = {
		mode: theme.mode,
		current: dark ? 'dark' : 'light'
	} satisfies Theme;
	return t;
};

export default function ThemeProvider({ children }: { children?: React.ReactNode }) {
	const [theme, setTheme] = useState<Theme>(() => {
		const store = localStorage.getItem('theme');
		if (store) {
			try {
				const t = calcTheme(JSON.parse(store) as Theme);
				localStorage.setItem('theme', JSON.stringify(t));
				document.documentElement.classList.toggle('dark', t.current === 'dark');
				return t;
			} catch (e) {}
		}

		return {
			mode: 'auto',
			current: 'light'
		};
	});
	const toggleTheme = useCallback((theme?: Omit<Theme, 'current'>) => {
		if (theme) {
			const t = calcTheme(theme);
			setTheme(t);
			localStorage.setItem('theme', JSON.stringify(t));
			document.documentElement.classList.toggle('dark', t.current === 'dark');
		} else {
			setTheme((curr) => {
				const t = calcTheme({ mode: curr.current === 'dark' ? 'light' : 'dark' });
				localStorage.setItem('theme', JSON.stringify(t));
				document.documentElement.classList.toggle('dark', t.current === 'dark');
				return t;
			});
		}
	}, []);
	return <ThemeContext.Provider value={{ theme, toggleTheme }}>{children}</ThemeContext.Provider>;
}

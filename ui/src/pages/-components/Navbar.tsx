import {
	Navbar as NextNavbar,
	NavbarBrand,
	NavbarContent,
	NavbarItem,
	Switch
} from '@nextui-org/react';
import { FiSun, FiMoon } from 'react-icons/fi';
import { useTheme } from '@/hooks/theme';

export default function Navbar() {
	const { theme, toggleTheme } = useTheme();
	return (
		<NextNavbar>
			<NavbarBrand>
				<p className="font-bold text-inherit">PromptKit</p>
			</NavbarBrand>
			<NavbarContent justify="end">
				<NavbarItem>
					<Switch
						isSelected={theme.current === 'light'}
						color="default"
						thumbIcon={({ isSelected, className }) =>
							isSelected ? <FiSun className={className} /> : <FiMoon className={className} />
						}
						onValueChange={(e) => toggleTheme({ mode: e ? 'light' : 'dark' })}
					/>
				</NavbarItem>
			</NavbarContent>
		</NextNavbar>
	);
}

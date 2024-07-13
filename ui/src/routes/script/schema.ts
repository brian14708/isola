import {
	type Infer,
	boolean,
	refine,
	number,
	min,
	defaulted,
	nonempty,
	object,
	string,
} from "superstruct";
import JSON5 from "json5";

export const dataSchema = object({
	code: string(),
	arguments: refine(string(), "JSON5", (s: string) => {
		try {
			JSON5.parse(s, () => undefined);
			return true;
		} catch {
			return `Invalid JSON5`;
		}
	}),
	method: nonempty(string()),
	timeout: min(number(), 0.1),
	stream: defaulted(boolean(), false),
});
export type Data = Infer<typeof dataSchema>;
export const DEFAULT_DATA: Data = {
	code: `def handle(request):
    return request`,
	arguments: `[
  { "value": 123 }
]`,
	method: "handle",
	timeout: 15.0,
	stream: false,
};

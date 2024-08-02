import { createPromiseClient, type PromiseClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import type { DescService } from "@bufbuild/protobuf";

const transport = createGrpcWebTransport({
	baseUrl: "/",
	useBinaryFormat: true,
	credentials: "include",
	interceptors: [],
});

const clientMap: Record<string, PromiseClient<DescService>> = {};
const isBrowser = typeof window !== "undefined";

export default function client<T extends DescService>(service: T): PromiseClient<T> {
	if (!isBrowser) {
		throw new Error("Cannot create client in non-browser environment");
	}
	if (service.typeName in clientMap) {
		return clientMap[service.typeName] as PromiseClient<T>;
	} else {
		const client = createPromiseClient(service, transport);
		clientMap[service.typeName] = client;
		return client;
	}
}

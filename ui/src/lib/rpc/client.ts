import { createClient, type Client } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import type { DescService } from "@bufbuild/protobuf";

const transport = createGrpcWebTransport({
	baseUrl: "/",
	useBinaryFormat: true,
	credentials: "include",
	interceptors: [],
});

const clientMap: Record<string, Client<DescService>> = {};
const isBrowser = typeof window !== "undefined";

export default function client<T extends DescService>(service: T): Client<T> {
	if (!isBrowser) {
		throw new Error("Cannot create client in non-browser environment");
	}
	if (service.typeName in clientMap) {
		return clientMap[service.typeName] as Client<T>;
	} else {
		const client = createClient(service, transport);
		clientMap[service.typeName] = client;
		return client;
	}
}

import { createRemoteJWKSet, jwtVerify } from "jose";

export interface AccessOptions {
  teamDomain: string;
  aud: string;
}

/**
 * Cloudflare Access puts a signed JWT on every request it lets through. Verifying it here
 * is what lets a browser session act without a shared action token: Access owns the login,
 * the hub owns the check. Requests that reach the hub by any other route (a tailnet peer
 * hitting Caddy directly) carry no valid assertion and stay unauthorized.
 */
export function createAccessVerifier(options: AccessOptions) {
  const issuer = `https://${options.teamDomain}`;
  const jwks = createRemoteJWKSet(new URL(`${issuer}/cdn-cgi/access/certs`));
  return async function verifyAccess(request: Request): Promise<string | undefined> {
    const assertion = request.headers.get("Cf-Access-Jwt-Assertion");
    if (!assertion) return undefined;
    try {
      const { payload } = await jwtVerify(assertion, jwks, { issuer, audience: options.aud });
      const email = payload.email;
      return typeof email === "string" && email ? email : undefined;
    } catch {
      return undefined;
    }
  };
}

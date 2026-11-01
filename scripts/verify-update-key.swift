// CI-only: confirm the private archive key matches the public key shipped in the app.
// No key material is printed. The private file must be mode 600 and removed after signing.
import Foundation
import CryptoKit

let encoded = try String(contentsOfFile: CommandLine.arguments[1], encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
guard let seed = Data(base64Encoded: encoded) else { fatalError("Invalid update signing key encoding") }
let key = try Curve25519.Signing.PrivateKey(rawRepresentation: seed)
let data = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2]))
let info = try PropertyListSerialization.propertyList(from: data, format: nil) as! [String: Any]
guard info["SUPublicEDKey"] as? String == key.publicKey.rawRepresentation.base64EncodedString() else {
    fatalError("Update signing key does not match the client's public verification key")
}

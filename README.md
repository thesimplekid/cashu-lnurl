# Cashu LNURL

Service that allows users to sign up for an ln address via nostr. 
Sats paid to this address are minted as cashu tokens of the users preferred mint and direct messaged to them via nostr.


# Zaps
To enable [zap](https://github.com/nostr-protocol/nips/blob/master/57.md) notes to be published some extra configuration is needed as well as a CLN node. This is because a valid zap request requires the invoice description to be a zap_request. In order to provide best privacy mints do not allow descriptions to be set.  

When zaps are enabled (proxy = true) this service uses the configured CLN rpc to create an invoice, that is returned to the when a request is made to the lighting address. Once this invoice is paid this service then requests a mint, mints a cashu token and sends a nostr direct message to the preconfigured pubkey. This could be improved in two ways, the first being make this a true wrapped invoice so the service cannot take funds they must pay the mint invoice, the second is use P2SH to lock the cashu token to only be readable by the nostr key it is being sent to. This reduced the trust in the service, though of course there is no way to know if the service is doing this for every invoice request, so there will always be some trust involved, though more temporary then a custodial wallet, as once the token is redeamed by the user there is no way for the service to claim it back or know what happens to it next.

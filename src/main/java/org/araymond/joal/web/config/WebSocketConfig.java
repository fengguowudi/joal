package org.araymond.joal.web.config;

import org.araymond.joal.web.annotations.ConditionalOnWebUi;
import org.springframework.beans.factory.annotation.Value;
import org.springframework.context.annotation.Configuration;
import org.springframework.messaging.simp.config.MessageBrokerRegistry;
import org.springframework.web.socket.config.annotation.EnableWebSocketMessageBroker;
import org.springframework.web.socket.config.annotation.StompEndpointRegistry;
import org.springframework.web.socket.config.annotation.WebSocketMessageBrokerConfigurer;
import org.springframework.web.socket.config.annotation.WebSocketTransportRegistration;

import javax.inject.Inject;

/**
 * Created by raymo on 22/06/2017.
 */
@ConditionalOnWebUi
@Configuration
@EnableWebSocketMessageBroker
public class WebSocketConfig implements WebSocketMessageBrokerConfigurer {
    // Allow large payloads (e.g., torrent files) to travel through the WebSocket channel without a tight cap.
    private static final int NO_MESSAGE_SIZE_LIMIT = Integer.MAX_VALUE;
    private final String webSocketPathPrefix;

    @Inject
    public WebSocketConfig(@Value("${joal.ui.path.prefix}") final String webSocketPathPrefix) {
        this.webSocketPathPrefix = webSocketPathPrefix;
    }

    @Override
    public void configureWebSocketTransport(final WebSocketTransportRegistration registration) {
        registration
                .setMessageSizeLimit(NO_MESSAGE_SIZE_LIMIT)
                .setSendBufferSizeLimit(NO_MESSAGE_SIZE_LIMIT);
    }

    @Override
    public void configureMessageBroker(final MessageBrokerRegistry config) {
        config.enableSimpleBroker(
                "/global",
                "/announce",
                "/config",
                "/torrents",
                "/speed"
        );
        // Message received with one of those destinationPrefixes will be automatically router to controllers @MessageMapping
        config.setApplicationDestinationPrefixes("/joal");
    }

    // Handshake endpoint
    @Override
    public void registerStompEndpoints(final StompEndpointRegistry registry) {
        registry.addEndpoint(this.webSocketPathPrefix)
                .setAllowedOrigins("*");
    }

}

package org.bezalel.fixture;

import net.fabricmc.api.ModInitializer;

public final class Fixture implements ModInitializer {
    @Override
    public void onInitialize() {
        System.out.println("Bezalel Fabric fixture loaded");
    }
}
